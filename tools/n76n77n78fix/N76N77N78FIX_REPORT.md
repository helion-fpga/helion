# N76/N77/N78 Fix Report

Branch: `fm/ultra-bitgen-n76n77n78fix`, merge tip `7e39aade`
(impl `6a7de419`, report commit pending below). Base: `9cd1c205`
(n74n75fix tip). Diff vs base: exactly 2 files, +384/-45
(`tools/ultra-bitgen/run-suites.sh` +208/-45,
`tools/n76n77n78fix/n76n77n78-prune-test.sh` +176, new). No production
code changed; script-only lane. No merges into fm/ultra-bitstream,
fm/zedboard-merge, or master.

Headline: N76, N77, N78 all CLOSED as fixed. The prune now deletes
0 files and frees 0 bytes on a post-suite target, which is the correct
result, not a failure: the 54 release and 3 debug old-key dup groups
are live dependency-universe pairs, and the fixed 6-field key
recognizes them as live and preserves all of them.

## 1. N76: CLOSED as fixed (safe orphan rule; true orphans 0)

W-INVEST reproduced the filed count exactly on the reference target
(73 release deps-orphans: 35 build-script + 36 run-build-script + 2
example; 50 debug deps-orphans: 25 + 25) and then proved the filing
was a mislabelled live set: the true orphan count under the full live
rule is 0 dirs and 0 bytes on every stable target inspected (reference,
n74n75fix-coord, critic54, intintf-model). Every one of the 71
build-script-kind deps-orphans has its hash as a
`release/build/<crate>-<hash>/` dir name, and the 2 example hashes
appear in `release/examples/` as `perf_profile-9bf07dccc42df14f` and
`hnf-db53dc4bdfec72a5`.

The fix (W-IMPL, commit 6a7de419) adds `sweep_fingerprint_orphans`
to run-suites.sh, implementing the 4-part rule: a fingerprint dir is
deletable only if its hash is absent from `<profile>/deps/`
filenames AND absent from `<profile>/build/` dir names AND absent
from `<profile>/examples/` filenames AND its JSON stem kind is not
build-script, run-build-script, or example. Kind comes from the JSON
stem matched against fixed kind prefixes in longest-first order
(run-build-script-, build-script-, test-integration-test-, test-bin-,
test-lib-, lib-, bin-, example-); the build-script stem trap
(`build-script-build-script-build`) is handled, and dirs with no JSON
or an unknown kind are never deleted. The sweep runs for release and
debug after each profile prune. A hash-only sweep would wrongly
delete 73 live release dirs (57,898 bytes) and 50 live debug dirs on
the reference target; the safe rule deletes 0.

Proof: synthetic test case (c) deletes a true orphan (hash nowhere,
kind lib) while case (d) keeps build-script, run-build-script, and
example dirs alive even with their hashes absent from deps/, build/,
and examples/ in the fake tree, so only the kind guard saves them.
Case (e): a hash-only sweep would wrongly delete 4 release dirs vs
the safe rule deleting 1 (debug 1 vs 0). On the real impl target the
sweep deleted 0/0 (release 141 deps-orphans, debug 50, all live); on
the coordinator target post-suite it deleted 0/0 as well.

## 2. N77/N78: CLOSED as fixed (deps-closure key)

W-INVEST corrected two prior claims with file evidence. First, the
N74/N75 lane report section 6 claim that the serde pair has
"identical fingerprint JSON" is wrong: the serde JSONs differ in the
serde_derive deps fourth value (12369178089602604572 vs
16723115377204561444). Second, the critic's "backtrace differs only
in the libc dependency fingerprint" is wrong: backtrace differs in
libc AND object fourth values (14326352615680267064 vs
591560418076643814, 203463601578939990 vs 7238470026232778237).
Consequently the "no static key available from the fingerprint JSON"
claim is also wrong: the deps array in each unit JSON is such a key.

The fix extends the prune group key from (crate, version, features,
unit-kind, profile) to (crate, version, features, unit-kind,
profile, deps-closure), where deps-closure is the unit JSON deps
array verbatim, normalized by sorting the entries and rendering each
as `<first-u64>:<dep-name>:<public-bool>:<fourth-u64>`, joined with
commas. The JSON `local` field is excluded (its dep_info path
carries the own hash). Extraction uses python3 reading the JSON, not
hand-parsed grep. The key never uses the sibling fingerprint-hash
file (the bare 16-hex file named after the JSON stem): it differs
between every pair member but is a hash of the whole fingerprint,
so keying on it would split stale rebuilds whose dep closure is
unchanged. No resolved transitive closure is attempted: no file on
disk maps a deps fourth-u64 value to a unit hash, and transitive
differences propagate into the direct vector, so the raw vector
suffices.

Separation proof (W-INVEST, replicated in Python over every unit in
each target): the deps vector separates all 54 release dup groups on
the post-suite targets and all 50 on the reference target, and all
3-4 debug groups, with zero groups sharing a vector. A genuinely
stale same-closure hash (identical 6-field key) would still fall in
one group and still be pruned.

Decisive experiment (W-IMPL, own worktree): full release build exit
0; baseline suites 475/0/1, 92/0/1, checkers 100 OK; pre-prune old
5-field dup groups 54 release / 3 debug, new 6-field dup groups 0 /
0, zero old-key groups sharing a deps vector; prune run exit 0 with
release deps unchanged at 2,657,199,831 bytes (freed 0), dup groups
0 to 0, files deleted 0, fingerprint dirs deleted 0, orphan sweep
0/0 in both profiles, suites after prune at the same totals;
rebuild (`cargo build --release --bin helion --bin helion-bitgen`
plus `cargo test --release -p helion-cli --no-run` plus `cargo test
-p helion-ultrascale --no-run`) all exit 0, the bin rebuild finished
in 4.57s with nothing recompiled, 0 deleted hashes reappeared,
fingerprint-dir counts (712/190) and deps-file counts (1662/373)
identical before and after the rebuild, post-rebuild dup groups new
key 0/0 and old key 54/3 with all pairs intact.

Synthetic test (30 checks, PASS, exit 0, log
`impl/synthetic-prune-test.log`): (a) same 5-field key, different
deps vector: BOTH universe members survive; (b) same 5-field key AND
same deps vector, older mtime: the older unit is deleted and the
newer kept, which also guards the `local` exclusion (the two units
differ only in the own-hash local dep_info path, so including
`local` would have wrongly kept both); (c)-(e) as in section 1. The
test extracts the exact shipped prune functions from run-suites.sh
by marker, never a hand copy. It also passes from the merged lane
branch (coordinator run, log `coord/synthetic-prune-test.log`).

## 3. Coordinator acceptance (own runs from merged branch 7e39aade)

- Release build exit 0 (helion 13,095,296 bytes md5 b52179c3,
  helion-bitgen 5,738,240 bytes md5 1ef1a876; the md5s differ from
  the N74/N75 lane's d3f4985a/a1ed3100 at identical sizes, which is
  build-to-build variation, N73/N54 recurrence, not a code change).
- run-suites.sh --prune-target exit 0, ALL SUITES DONE: ultrascale
  475/0/1, CLI release 92/0/1, checkers Ran 100 tests OK. Prune
  freed 0 bytes in both profiles, dup groups 0 to 0, orphan sweep
  0/0. Post-suite release deps is 1,141,266,478 bytes with 0 new-key
  dup groups: the fix holds on a post-suite target that now contains
  the test universe.
- 12 flows exit 0 with numbers identical to the n69n72fix tip:
  chain16 8570/362/4cf99124 twice, shift32 8690/289/a7a070dd twice,
  counter12 7370/752/b18112d3, counter8 s0 8570/363/2902d6f0, s1
  8570/342/01cb7f90, counters2x4 8450/307/730fcc93, counter5
  9650/82/6fe2c80a, counter 9650/58/4c99a51f, blinky 9650/4/d82b70c8,
  counter16 5750/1278/a45c6efd.
- 12 verify-bits PASS (800/0 each, counter16 80000/0), decoder
  built from this branch with US09V_DECODER_BIN set (N34 guard).
  Blinky .bit md5 d82b70c8e29ce72f28e08c94f3a16d16 unchanged.
- bash -n passes on run-suites.sh and on the synthetic test script.
- One VM restart killed the first acceptance suite run mid-CLI-suite
  (ultrascale half already done at 475/0/1, no process survived);
  the run was re-launched from scratch and completed exit 0 end to
  end. No results are taken from the killed run.

## 4. New nits filed

- N79 (non-blocking): prune runtime cost. `artifact_unit_info`
  spawns python3 once per deps-file row (up to 3 rows per unit), so
  `count_dup_crates` takes about 1 minute per profile and the full
  prune phase about 6-7 minutes for both profiles. Correctness is
  unaffected. A per-(crate, hash) cache of the unit info would cut
  it roughly 3x.
- N80 (trivial, non-blocking): the orphan sweep keeps its hash
  lists in a `mktemp -d` dir under /tmp. It deletes that dir at the
  end of every sweep and recomputes it on the next run, so a restart
  mid-sweep only leaks a few small files. No logs live in /tmp.

## 5. Corrections carried to the critic

- The N74/N75 lane report section 6 "identical fingerprint JSON"
  claim for serde is wrong (section 2 above).
- The Critic Session 53/54 filing "backtrace differs only in the
  libc dependency fingerprint" is wrong (it differs in libc AND
  object).
- The "no static key available from the fingerprint JSON" claim is
  wrong (the deps array separates every observed pair).
- N74 stays closed: the named N74 cases (syn versions, memchr
  feature sets) are unaffected by this lane, and the suites pass at
  the same totals.

## 6. Logs and evidence

- Lane logs: `~/workspace/n76n77n78fix-logs/` (`invest/`, `impl/`,
  `coord/`).
- W-INVEST report: `n76n77n78fix-logs/invest/N76N77N78-INVEST.md`
  (509 lines, 0 em dashes).
- W-IMPL report: `n76n77n78fix-logs/impl/N76N77N78-REPORT.md`
  (synthetic test log, build log, baseline logs, pre-prune dup
  measurements, prune-run logs, rebuild logs, post-rebuild dups).
- Coordinator acceptance: `n76n77n78fix-logs/coord/`
  (`build-release.log`, `suites-console.log`, `suite-summary.txt`,
  `flows-console.log`, `coord-flow-summary.txt`,
  `verify-console.log`, `coord-verify-summary.txt`,
  `synthetic-prune-test.log`, `coord-run-flows.sh`,
  `coord-run-verify.sh`).
