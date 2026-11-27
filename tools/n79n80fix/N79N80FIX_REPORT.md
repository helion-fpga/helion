# N79/N80 fix report

Branch: `fm/ultra-bitgen-n79n80fix`
Worktree: `~/workspace/helion-ultra-bitgen-n79n80fix`
Base: `fm/ultra-bitgen-n51n53n59n63fix` at
`6fb14f38bcb395634934dd440b483ded8b369241`, verified before branching:
`git rev-parse` in the base worktree and `git ls-remote origin
fm/ultra-bitgen-n51n53n59n63fix` both read the same hash. No
`fm/ultra-bitgen-n79*` ref existed on origin and no n79 worktree
existed. No merges, no critic dispatched.

Both nits are from Critic Session 55 (CRITIQUE.md lines 7277-7281),
confirmed real by the critic's own runs. Both fixes are in one file
only: `tools/ultra-bitgen/run-suites.sh` (+65/-2). No Rust code, no
N7/N8/N9/Q code, no decoder behavior, no N34/N37/N38 checks touched.
The N9 worktree and branch were not touched. The N76/N77/N78 lane
test script was not touched (it was run, not modified).

Commits (one fix per commit):

- `7b0a80e6` n79fix: cache artifact deps closures, one python3
  precompute per prune run
- `351c8097` n80fix: trap SIGTERM so the orphan sweep tmp dir is
  removed on kill

## Reference target

All prune timings and decision comparisons use the same reference
target on the same machine: `~/workspace/helion-critic55-base/target`,
the critic's post-suite target. Verified byte-identical to the critic's
post-suite state before the control run: release deps 1,137,820,586
bytes / 611 files, debug deps 2,505,932,250 bytes / 373 files, matching
the critic's numbers exactly. On this target the base (new-key) prune
deletes 0 files and 0 fingerprint dirs on both profiles, which is the
correct result (the pairs are live universe pairs), so control and
after runs are directly comparable with no rebuild in between.

## N79: prune runtime cost

### The cost

`artifact_deps_closure()` spawned `python3 -c` once per (crate, hash)
row, and it is called once per row in three passes per profile prune
(the main prune loop plus `count_dup_crates` before and after), so
about 2,952 python3 spawns per prune phase on the reference target
(611 release + 373 debug rows, times 3 passes).

### The fix

`prune_target` now precomputes every unit JSON deps closure in one
python3 process per profile into `$LOGDIR/deps-closure.cache` (476
lines on the reference target: 476 unit JSONs), and
`artifact_deps_closure` reads it with one grep per row, falling back
to the python3 computation on a cache miss. Cache lines are
`<json-path>\t<closure>`, keyed by absolute JSON path, so the two
profiles never collide, and unit JSONs are immutable while the prune
runs. Standalone callers that never set `DEPS_CLOSURE_CACHE` (the
N76/N77/N78 synthetic test calls `prune_profile_dir` directly) behave
exactly as before through the fallback path. What gets pruned is
unchanged; only how fast the closure is computed.

### Control timings (base code, prune phase, both profiles)

Measured with the control harness
(`~/workspace/ultra-bitgen-crew/logs/n79n80fix/control-prune.sh`),
which runs the shipped prune functions on the reference target. Two
independent control runs:

| run | prune release | sweep release | prune debug | sweep debug | total |
|-----|---------------|---------------|-------------|-------------|-------|
| control 1 | 97.52s | 2.01s | 62.74s | 1.35s | 163.63s |
| control 2 | 96.82s | 2.00s | 60.21s | 1.33s | 160.36s |

Control mean: 162.0s. Both controls deleted 0 files, 0 fingerprint
dirs, freed 0 bytes on both profiles. This matches the critic's
2m44s run (163.63s vs 164s), confirming the same machine and target.

### After timings (fixed code, through the shipped `prune_target`)

Measured with
(`~/workspace/ultra-bitgen-crew/logs/n79n80fix/after-prune-target.sh`):

| run | prune_target total |
|-----|--------------------|
| first after run | 262.38s |
| after 1 | 99.59s |
| after 2 | 95.95s |
| after 3 (final code) | 60.98s |

After median: 97.77s. Conservative reading (median after vs mean
control): 162.0s to 97.77s, about 40% faster, about 64s saved, 1.7x.
The 60.98s run is a fast outlier from shared-CPU variance on this VM
and is not claimed as typical. The two control runs cluster tightly
(160-164s) and the after-target runs cluster tightly (96-100s),
so the reduction is real and not measurement noise. Correction (N83):
the first after run at 262.38s
(`~/workspace/ultra-bitgen-crew/logs/n79n80fix/after/timings.txt`)
was omitted from this list in the first version of this report;
including it moves the median from 95.95s to 97.77s and the headline
from about 41% to about 40% faster, so the conclusion stands.

### Micro-benchmarks

Direct equivalence check over all 476 unit JSONs in the reference
target (`verify-n79-cache.sh`): cached closure equals direct python3
closure for 476/476 JSONs, 0 cache misses. Precompute wall: 0.77s
for all 476 JSONs. Cached-only lookups: 1.50s vs direct-only
lookups: 17.99s for the same 476 JSONs (12x per lookup).

### Prune-decision identity proof

Prune decisions are byte-identical between control and after. All 8
prune artifacts compare identical with `cmp` (control vs after 1, 2,
and 3, and control 1 vs control 2):

- `prune-release.list` (611 rows), `prune-debug.list` (373 rows)
- `prune-release.delete`, `prune-debug.delete` (both empty)
- `prune-release.pairs`, `prune-debug.pairs` (both empty)
- `orphan-release.delete`, `orphan-debug.delete` (both empty)

Summary lines apart from timing are identical on every run: release
deps 1,137,820,586 to 1,137,820,586 bytes (freed 0), dup groups 0 to
0, files deleted 0, fingerprint dirs deleted 0; debug deps
2,505,932,250 to 2,505,932,250 bytes (freed 0), dup groups 0 to 0,
files deleted 0, fingerprint dirs deleted 0; both orphan sweeps
deleted 0 dirs, freed 0 bytes. Target snapshots (deps file lists and
fingerprint dir lists, before vs after) are identical in every run.

The first after run went through the fallback path only (direct
`prune_profile_dir` calls, cache never set, 262.38s under concurrent
load from the synthetic test and a scratch-tree build running at the
same time). Its decisions are also byte-identical to control. It is
listed in the after timings above; the first version of this report
omitted it, which N83 corrects.

## N80: orphan sweep tmp dir

### The defect

`sweep_fingerprint_orphans` created its 3 hash files in a `mktemp -d`
dir and removed it only at the end and on the rm-failure error path.
No trap existed in the file (`grep trap` returned nothing), so a kill
mid-sweep left the tmp dir behind.

### Control: kill test on base code (leak demonstrated)

Scratch target with 4,000 true orphan fingerprint dirs (kind lib,
hash absent from deps/, build/, examples/) plus 1 kept dir, under
`~/workspace/ultra-bitgen-crew/logs/n79n80fix/scratch-base`
(`kill-driver.sh`, `kill-sweep.sh`): SIGTERM sent mid-sweep with 29
audit lines written. Process dead, wait status 143. Tmp dir
`/tmp/tmp.5RDTKDVoUL` LEAKED: still present afterwards holding
`deps.hashes` (17 bytes), `build.hashes` (0 bytes),
`examples.hashes` (0 bytes). 30 dirs deleted before the kill, 3,971
remaining. Normal base runs remove the tmp dir: no `/tmp/tmp.*` dirs
remained after either control prune run.

### The fix

The sweep now traps SIGTERM (`trap 'rm -rf -- "$tmp"; exit 143'
TERM`) right after `mktemp -d`, and clears the trap on every return
path (normal end and rm-failure path) so it never fires outside the
function and never disturbs a caller's own traps.

SIGINT is deliberately NOT trapped. A non-interactive bash without job
control ignores SIGINT, so an INT trap can never fire in this script.
Proven by a minimal repro (`int-repro.sh`, same trap shape, loop of
100 sleeps): SIGTERM fires the trap at once (exit 143); SIGINT is
swallowed and the loop runs to completion (exit 0, all 100 ticks). A
SIGINT kill test on the fixed sweep confirms the same: the sweep
completed all 4,000 deletions and exited 0, because bash never
delivered the signal. An INT trap would be dead code; it was written,
tested, and removed on that evidence, and the N80 commit was amended
to the TERM-only form before push.

No EXIT trap is set on purpose: it would replace a caller's EXIT trap
(the N76/N77/N78 synthetic prune test relies on its own).

### Kill-test proof on fixed code (TERM)

Two independent TERM kill tests on the fixed code, same driver.
Correction (N83): kill-after2 did not start from a fresh 4,000-orphan
scratch tree. Its kill-result.txt says "orphan dirs before: 3980"
(`~/workspace/ultra-bitgen-crew/logs/n79n80fix/kill-after2/kill-result.txt`
line 3), the leftover of kill-after (4,001 before minus 21 deleted),
so kill-after2 reused the kill-after tree. The trap proof itself
holds (tmp dir gone, exit 143 on both fixed runs).

| run | audit lines at kill | process | wait status | tmp dir after kill | dirs deleted / remaining |
|-----|---------------------|---------|-------------|--------------------|--------------------------|
| kill-after (TERM+INT code) | 20 | dead | 143 | CLEAN, gone | 21 / 3,980 |
| kill-after2 (final TERM-only code) | 30 | dead | 143 | CLEAN, gone | 30 / 3,950 |

Normal runs on the final code also remove the tmp dir: no
`/tmp/tmp.*` dirs remained after any of the three after-target prune
runs or either synthetic test run. The 3 hash files and the sweep
behavior are otherwise unchanged (0 dirs deleted, 0 bytes freed on
the reference target; the synthetic test's true orphan is still
deleted and the protected kinds still survive, see below).

## Regression: N76/N77/N78 synthetic prune test

`tools/n76n77n78fix/n76n77n78-prune-test.sh` passes fully against the
fixed `run-suites.sh`, exit 0, `ALL SYNTHETIC PRUNE TESTS PASSED`,
run twice (once on the TERM+INT code, once on the final TERM-only
code). It exercises the shipped prune functions standalone with
`DEPS_CLOSURE_CACHE` unset, so it proves the fallback path and the
trap changes do not break the prune or the sweep, and its own EXIT
trap survives the sweep's TERM trap (its work dir is cleaned).

## Residuals

- R1: remaining prune time (about 96-100s typical) is dominated by
  the per-row `artifact_version` greps and the sed pipelines in
  `artifact_unit_info`, not by python3. Batching or caching those is
  a separate optimization, outside N79 scope; N79 removed only the
  per-row python3 spawn, as filed.
- R2: SIGKILL and power loss still leak the sweep tmp dir; no trap
  can cover those. Also the sub-millisecond window between `mktemp
  -d` and the trap setup. Both are trivial (3 small hash files).
- R3: timing variance on this shared VM is large (after runs 61s to
  262s); the conservative median-vs-mean reading (about 40% faster)
  is the claim, not the fastest run.
- R4: SIGINT is not covered, by bash design (see N80 section). TERM
  is the covered kill signal, which matches the critic's filed case
  (VM restart, SIGTERM).
- R5: the cache file `$LOGDIR/deps-closure.cache` is a new log
  artifact (476 lines on the reference target). It is recreated fresh
  on every `prune_target` run.
- R6: the fallback-path after run (262.38s) ran under concurrent load;
  it is listed in the after timings above (N83 correction) and its
  decisions are byte-identical to control.

## Verification logs

All logs under `~/workspace/ultra-bitgen-crew/logs/n79n80fix/`:

- `control/`, `control2/`: control prune runs (base code), timings,
  prune tables, before/after snapshots, extracted functions.
- `after/`: first after run, fallback path (262.38s, listed in the
  after timings above).
- `after-target/`, `after-target2/`, `after-target3/`: after prune
  runs through the shipped `prune_target` (after-target3 is the
  final code); each holds `timings.txt`, `suite-summary.txt`,
  `prune-*.list/.delete/.pairs`, `orphan-*.delete`,
  `deps-closure.cache`, before/after snapshots.
- `n79cache/`, `n79time/`: closure equivalence check (476/476) and
  the cached-vs-direct lookup micro-benchmark.
- `kill-base/`, `kill-after/`, `kill-after2/`: TERM kill-test results
  (`kill-result.txt`, `kill-sweep-console.log`).
- `kill-after-int/`: SIGINT kill-test result (sweep completes, exit
  0; bash swallows SIGINT).
- `int-repro.log`, `term-repro.log`: minimal SIGINT-vs-SIGTERM trap
  repro.
- `run-suites-base.sh`: base `run-suites.sh` at 6fb14f38, verified
  byte-identical to the base worktree file.
- `scratch-base/`, `scratch-after/`, `scratch-after-int/`: synthetic
  4,000-orphan target trees used by the kill tests.
- `control-prune.sh`, `after-prune-target.sh`, `kill-sweep.sh`,
  `kill-driver.sh`, `kill-driver-int.sh`, `build-scratch.sh`,
  `verify-n79-cache.sh`, `time-lookup.sh`: harnesses.
