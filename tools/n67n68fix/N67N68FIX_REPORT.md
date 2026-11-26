# N67/N68 fix lane report

Lane branch: fm/ultra-bitgen-n67n68fix, code tip 2aafeb60 (impl commit
on intintf-model base 633327af). No merges into fm/ultra-bitstream,
fm/zedboard-merge, or master. No critic dispatched by this lane; Critic
Session 52 is the main agent's to dispatch.

## 1. Outcome

Both nits from Critic Session 51 are fixed and verified by independent
runs at every level (baseline, impl, coordinator):

- N68 is CLOSED. `blinky_routes_on_loaded_tileconn_map` now passes for
  real: tile_count 4, wire_count 5900, 8/8 in the file in about 65s.
  The honest ultrascale suite total with HELION_TILECONN_DB_ROOT set is
  now 475 passed, 0 failed, 1 ignored, and the tileconn_route tests run
  for real instead of skipping vacuously, because the canonical suite
  script exports the variable.
- N67 is CLOSED. The GAP-F5 mislabel is gone from the intintf-model
  report (both copies, byte-identical) and from the COORD.md COMPLETE
  entry for that lane. The report now says the preliminary xczu3eg STA
  DB has no documented DSP/BRAM delays.

What this lane does NOT do: no production code changes, no flow number
moves, no DSP/BRAM end-to-end work (still blocked on placement,
config-bit encoding, and INT_INTF segbits, as Session 51 ruled).

## 2. The fix (3 files, +63/-4)

- `crates/helion-ultrascale/tests/tileconn_route.rs` (2 lines):
  `blinky_routes_on_loaded_tileconn_map` now asserts tile_count 4 and
  wire_count 5900 (was 1 and 1475). Nothing else in that file changed.
- `tools/ultra-bitgen/run-suites.sh` (new file, 59 lines, mode 100755):
  canonical suite runner. Takes WORKTREE and LOGDIR as arguments
  (defaults mirror the prior lane scripts). Exports PATH including
  ~/.cargo/bin and ~/workspace/oss-cad-suite/bin. Requires
  US09V_DECODER_BIN to point at an executable decoder built from the
  current branch and fails loud naming N34 if it is unset or not
  executable. Runs serially with -j1: ultrascale suite (cargo test
  --no-fail-fast -j1 -p helion-ultrascale), CLI release suite (cargo
  test --release -j1 -p helion-cli), US-09 checkers (python3 -m
  unittest discover -s tests from tools/us09v). Prints a summary with
  passed/failed/ignored counts and exits nonzero if any suite fails.
  The tileconn DB variables (HELION_TILECONN_DB_ROOT,
  HELION_TILECONN_PART) are exported ONLY for the ultrascale suite and
  unset before the CLI suite and checkers. The reason is in the script
  header and in section 5: with the map loaded, three helion-cli ultra
  tests fail because they assume no pin-wire map is loaded.
- `tools/intintf-model/INTINTF_MODEL_REPORT.md` (2 lines in each of two
  places): section 9 item 3 and section 1 now say the preliminary
  xczu3eg STA DB has no documented DSP/BRAM delays; the GAP-F5 label
  is dropped. The crew copy at
  ~/workspace/ultra-bitgen-crew/INTINTF_MODEL_REPORT.md was changed
  identically; `cmp` exits 0, and `grep` finds zero GAP-F5 matches in
  either copy.

No other file changed. No production code changed anywhere.

## 3. Baseline (W-BASELINE)

Base tip 633327af, worktree clean, nothing committed. Report
~/workspace/n67n68fix-logs/baseline/BASELINE_REPORT.md (274 lines, 0 em
dashes).

- N68 fail-before: tileconn_route for real (DB root and part set) gives
  exit 101, 7 passed, 1 failed, in 66.21s. `blinky_routes` panics at
  tileconn_route.rs:208 with tile_count 4 vs the asserted 1. The
  wire_count assertion never runs, so 5900 was unproven at baseline; it
  is the critic's proposed value, not an observed one.
- Honest suite totals on base: with the DB set 474 passed, 1 failed, 1
  ignored (assembled: plain cargo test stops at the first failing
  binary, so the unreached timing_db binary was run separately, 12/0/0,
  plus doc tests 0/0/0); without the DB set 475/0/1 with tileconn_route
  passing 8/0/0 in 0.01s, the vacuous skip.
- N67: report section 9 item 3 line 192, section 1 line 21, and the
  COORD.md honest-limits line 6369 quoted verbatim, all using the
  GAP-F5 label. GAP-F5 closed at STA by Session 32 (CRITIQUE.md line
  5469; GAPF5_STA_REPORT.md line 7).
- Skip pattern confined to tests/tileconn_route.rs (zero matches in
  src/); the 8 skipping tests are listed by name and line in the
  baseline report. No suite-runner script was tracked in the repo
  (docs/ultra-bitgen-crew/gate.sh is an old integration gate, no cargo
  test); the 14 per-lane run-*-suites.sh files in lane log dirs do not
  export HELION_TILECONN_DB_ROOT (grep count 0 in each).

## 4. Impl acceptance (W-IMPL, branch tip 2aafeb60)

Report ~/workspace/n67n68fix-logs/impl/IMPL_REPORT.md (274 lines, 0 em
dashes).

- N68 probe: with tile_count fixed to 4 and wire_count still 1475, the
  run panics at line 209 with left 5900, right 1475. Observed wire_count
  is 5900, exactly the critic's proposal, confirmed by observation, not
  assumed. Final assertions 4 and 5900; tileconn_route passes 8/8 for
  real in 65.23s standalone, 66.34s inside the full suite.
- Scoped release build exit 0 in 9m 16s (resumed run): helion
  13,095,296 bytes md5 08f8b7050bb9e07de346507b65154d3f, helion-bitgen
  5,738,240 bytes md5 3af9ba4feeff589dea2b24b28d1ee56c.
- Suites, serial, -j1: ultrascale with DB set, --no-fail-fast,
  475/0/1 (36 result lines); CLI release, DB vars unset, 92/0/1;
  checkers 100 OK in 112.041s (decoder built from this branch).
- rustfmt --check edition 2024 on tileconn_route.rs: base and impl both
  exit 1 with 8 pre-existing hunks; normalized outputs differ only in
  the two changed lines. No new deviations.
- Script guard verified: unset US09V_DECODER_BIN exits 1 with the N34
  message; `bash -n` syntax check passes.

## 5. The script scoping finding (why the DB variables are not global)

The first script version exported the DB variables at the top and ran
all three suites with them set. The CLI suite then failed 3 tests
(log suite-cli-release-with-db.log, 45 passed, 3 failed, fail-fast
partial): `blinky_stops_at_route_without_pin_wire_map` and
`counter_stops_at_route_without_pin_wire_map` expect the flow to fail
at ROUTE with no wire, but with the map loaded the flow succeeds, so
`unwrap_err()` panics on an Ok value at ultra.rs:2810; and
`counter_flow_emits_bitstream_on_xczu3eg` asserts every PIP is on
INT_X1Y0, but with the map loaded the route emits a PIP on INT_X2Y0
(panic at ultra.rs:3340). The script was fixed to export the variables
only for the ultrascale suite and unset them before the CLI suite and
checkers. The checkers do not read these variables (they use
US09V_DB_ROOT or a default path), so they run unset as in the prior
lanes. This scoping is the correct design: the variables are a
test-environment switch for the ultrascale tileconn tests, and leaking
them changes CLI flow behavior.

## 6. Coordinator acceptance (merged branch 752ce8d5, own runs)

Release build exit 0 in 3.43s (fingerprints fresh; the merged tip has
identical Rust sources to the impl tip): helion 13,095,296 bytes md5
08f8b7050bb9e07de346507b65154d3f, helion-bitgen 5,738,240 bytes md5
3af9ba4feeff589dea2b24b28d1ee56c, identical to the impl worker's build.
Binaries live in the shared target dir
~/workspace/helion-ultra-bitgen-intintf-model-impl/target/release/ (see
section 8).

The canonical script ran end to end as one invocation from the merged
branch (US09V_DECODER_BIN set to the merged-tip decoder,
CARGO_TARGET_DIR set to the shared baseline target dir): exit 0, ALL
SUITES DONE. Ultrascale 475/0/1, CLI release 92/0/1, checkers 100 OK in
124.556s. The tileconn_route binary ran for real inside the suite
(8/8 in 70.05s). This closes the gap the impl worker left: the script
had not been run end to end as one invocation before this run.

12 flows exit 0 with numbers identical to the intintf-model lane
(chain16 8570/362/4cf99124 twice, shift32 8690/289/a7a070dd twice,
counter12 7370/752/b18112d3, counter8 s0 8570/363/2902d6f0, s1
8570/342/01cb7f90, counters2x4 8450/307/730fcc93, counter5 9650/82/
6fe2c80a, counter 9650/58/4c99a51f, blinky 9650/4/d82b70c8 unchanged,
counter16 5750/1278/a45c6efd). 12 verify-bits PASS (800/0 each,
counter16 80000/0), decoder built from the merged branch, absolute
workdirs, --top passed. No flow number moved.

Fail-before proof, coordinator's own run: on the base worktree
(633327af), tileconn_route for real gives exit 101, 7 passed, 1
failed, blinky_routes panics at line 208 with left 4, right 1
(log tileconn-route-base-real2.log). With the fix, 8/8 in 70.05s.

rustfmt --check edition 2024 on tileconn_route.rs, lane vs base: both
exit 1 with 8 hunks; normalized outputs differ only in the two changed
lines. Zero new deviations.

Branch diff 633327af..752ce8d5 is exactly 3 files, +63/-4. The
report copy (227 lines) is not in that diff; it arrives in commit
f9fb18e7 (252 is the INTINTF report's line count). No other file touched.

## 7. New nits

- N69 (test only, pre-existing, found by this lane): helion-cli ultra
  flow tests use fixed temp dirs /tmp/helion-us09-flow-{blinky,counter}
  and assert the .bit/.fasm outputs do not exist after a ROUTE failure,
  but nothing cleans the directory first. A prior successful flow run
  (for example with HELION_TILECONN_DB_ROOT set, which makes the flow
  succeed) leaves outputs behind, and the next no-map run then fails on
  the existence assertion even though the flow correctly fails at
  ROUTE. Evidence: the impl worker's with-DB run wrote the files, the
  next no-DB run failed at ultra.rs:2819 on exactly that assertion,
  and deleting the four stale dirs made the suite pass 92/0/1. Suggested
  fix, out of this lane's scope: clean the dir at the start of the
  assert helper, or use a unique temp dir per run. N69 did not bite the
  coordinator's acceptance run: the stale dirs were removed before the
  script ran.
- N70 (environment, not code): shared target-dir reuse across commits
  accumulates duplicate-hash artifacts. The intintf-model-impl release
  dir grew 682M to 1.2G during the impl worker's verification because
  the c8099849 build's artifacts sit next to the rebuild's. A future
  lane should reclaim the stale hashes (for example `cargo sweep` or
  deleting the old-hash files) or use per-lane target dirs where disk
  allows.

## 8. Incidents

- Runtime restart killed the coordinator's first end-to-end script run
  mid-CLI-suite (uptime 0 min afterward; the CLI log ended inside the
  corpus_1000 test, 1 result line). The script was re-run from scratch;
  cargo reused fingerprints and the re-run finished exit 0 with ALL
  SUITES DONE. The killed run's partial log was overwritten by the
  re-run; the incident is recorded here, not in a log file.
- Disk quota: the impl worker hit "Disk quota exceeded (os error 122)"
  on a fresh target dir and verified via shared target dirs instead.
  The coordinator reclaimed 5.8G by removing only the build cache
  (target/) of the completed intintf-model-baseline worktree (sources
  untouched, worktree clean at 4adaeb35); a 1G fallocate then succeeded.
  The canonical script end-to-end run used the shared baseline target
  dir for this reason; its CLI release step built fresh release
  artifacts there (target grew 3.1G to 3.7G).
- The coordinator's first fail-before attempt ran the STALE fixed test
  binary from the shared target dir (shared-target fingerprint trap:
  the baseline worktree's older file mtime looked fresh to cargo), so
  it passed 8/8 and was discarded. The file was touched to force a
  rebuild from the base sources, and the re-run failed 7/1 as expected.
  The stale-binary run's log was overwritten by the valid re-run; the
  incident is recorded here.
- The impl worker's first CLI suite attempt used the wrong PATH (cargo
  not found, exit 127) in one coordinator-side invocation; re-run with
  ~/.cargo/bin on PATH. No result was taken from the failed attempt.

## 9. Carried

N65 (coord scripts duplicated fixture paths; this lane's own scripts
live under ~/workspace/n67n68fix-logs/coord/ and use the corrected
port-map paths), N51, N53, N52, N54, N55, N56, N59, N60, N61, N62, N63,
IOB blocked on B1, known-INIT datum, documented xczu3eg timing except
the single BRAM field, 54,907 non-band bits, N16, N9, N34, N37, N38,
A-CTRL, A-F7F8. Fabric delays still blocked on Vivado report_timing
plus write_sdf; Tier 2 waits for Vivado. No critic dispatched by this
lane; main agent dispatches Critic Session 52.
