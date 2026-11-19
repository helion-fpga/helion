# Multiseed timing closure: verification record (GAP-F6)

Branch: `fm/ultra-multiseed`, staging tip `e1c2e47`, worktree
`~/workspace/helion-ultra-multiseed`. No merges, no pushes. Vivado
license state UNKNOWN; nothing here runs Vivado or claims Tier 2.

## What the inherited code is (verified by reading, 2026-11-19)

- The multiseed proof is exactly one commit, `e1c2e47`
  ("feat(cli): overnight multi-seed timing-closure flow"), touching
  only `crates/helion-cli/src/main.rs` (+372 lines).
- `helion multiseed <design> --part P --seeds N --seed-start S --jobs J
  --out FILE --threshold T`:
  - Synths once (`synth_any`), then runs full `compile_design` per
    seed on a rayon pool of J threads.
  - Each seed appends one JSONL record
    `{seed, wns_ps, tns_ps, runtime_s, status, reason}` under a file
    mutex, flushed per record, so a killed run keeps finished seeds.
  - Resume: seeds already in the JSONL are skipped.
  - Best seed: highest `wns_ps` among `status == "ok"` and
    `wns_ps >= threshold`; ties break on lowest `tns_ps`, then
    lowest seed. Deterministic over the JSONL on disk.
  - 6 unit tests: best-seed tie-breaks, threshold/filter, JSONL
    roundtrip + escaping, checkpoint parsing.
- Seed mechanism (verified in `crates/helion-place/src/lib.rs`):
  `PlaceOpts.seed` drives `seeded_order`, a SplitMix64 Fisher-Yates
  shuffle of the LUTFF processing order. Seed 0 is the legacy order.
  The shuffle applies only when the timing guide carries no
  criticality info; with default (empty) constraints, as `multiseed`
  uses, it applies. Sites are recorded by packed index, never in
  visitation order, so the shuffle cannot corrupt INIT/router
  indexing.
- Per-seed work inside `compile_design` is pack + place + route +
  DRC + bitgen + STA. Pack and bitgen are seed-independent and repeat
  per seed; only place (via the seeded order) and its downstream
  route/STA vary with the seed.

## What is NOT on this branch (verified by listing/grep)

- No `crates/helion-ultrascale` crate and no `xcku3p`/`xcvu3p` part
  file anywhere in the tree. `Device::load_part` reads
  `devices/helion/parts/<name>.toml`; only HL10T/HL10M/HL10S parts
  exist. `helion multiseed --part xcku3p` therefore fails at device
  load before any seed runs.
- No UltraScale pack/place/route/STA staging is ported here
  (fm/ultra-packplace, fm/ultra-router, fm/ultra-timing live on
  their own staging branches). The driver is part-agnostic: it
  sweeps whatever `compile_design` supports for the named part.

## Proven vs blocked (status at staging tip)

- PROVEN (mechanism, generic part): seed sweep, JSONL checkpoint
  and resume, deterministic best-seed selection, per-seed failure
  recording. Reproduction run on HL10T below.
- NOT PROVEN: the GOAL.md "10-seed, WNS 8460 to 8820" claim has no
  run log attached to the commit; reproduction is pending.
- BLOCKED on xcku3p: 100-seed flow cannot run until GAP-F7 (device
  model / xcku3p part) plus GAP-F2/F3/F4/F5 (pack/place/route/STA
  on that part) land. This driver must not invent a device model,
  delays, or routing to work around that.
- The 100-seed flow itself is unverified until an end-to-end run
  happens on xcku3p; this branch can only harden the driver
  mechanics on the generic part.

## Defects found by reading (to fix on this branch)

1. Pack and bitgen repeat per seed although neither depends on the
   seed. For 100 seeds that is 99 wasted packs and 99 wasted
   bitgens. Fix: pack once, run place/route/STA per seed, bitgen
   only the best seed.
2. No per-seed logs. Only a one-line stderr summary per seed and
   the JSONL record. Fix: `--log-dir` with one log file per seed
   (placement/route/STA summary).
3. `file.lock().unwrap()` panics the whole sweep if the mutex is
   poisoned (a panicking worker thread kills every other seed).
   Fix: recover the inner file on poisoning.
4. The end-of-run report dedups by seed keeping the FIRST record.
   If a seed ever has two records (concurrent runs sharing one
   out file, or a hand-edited file), the stale one wins. Fix:
   last record wins.
5. `to_jsonl` escapes backslash, quote, and newline but not CR,
   tab, or other control characters. Fix: escape all control chars.
6. No best-seed artifact: the best seed is only printed, so
   reproducing it needs a manual `helion impl --seed N` rerun.
   Fix: `--best-out` writes the best seed, its numbers, and the
   exact reproduce command; `--emit-best` re-runs the full
   compile for the best seed and writes its bitstream.
7. Seed-range overflow: `seed_start + n_seeds` can wrap for
   extreme inputs. Fix: checked arithmetic, fail loud.

## Reproduction runs

(Commands and outputs appended here as they are run. Nothing in
this file is claimed without a shown run.)

Binary: `target/debug/helion`, built 2026-11-19 with
`cargo build -j1 -p helion-cli --bin helion --offline`
(3m 00s, 2 pre-existing rustc warnings in `main.rs`: unused
imports `place_with`, `route_with`, also present at e1c2e47).
Tests: `cargo test -j1 -p helion-cli --offline --bin helion`
(bin target holds only the 16 multiseed tests): 16 passed,
0 failed, including the 4 new tests from d2db932a.
Clippy: `cargo clippy -j1 -p helion-cli --offline --bin helion
--all-targets` cannot reach helion-cli: it stops in `helion-sv`
with `clippy::overly_complex_bool_expr` denied by default at
`crates/helion-sv/src/lib.rs:15059` (`descending || (!ne_cmp &&
step_signed < 0) || (ne_cmp && step_signed < 0)`), plus 426
warnings in that crate. Verified pre-existing: the identical
line is at e1c2e47 and this branch has zero diff under
`crates/helion-sv/`. Out of GAP-F6 scope, so not fixed here;
clippy on helion-cli stays blocked on it.

### Run 1: 5-seed sweep, counter, HL10T (2026-11-19 ~17:29 UTC)

Command: `./target/debug/helion multiseed examples/counter.sv
--part HL10T-C32-1 --seeds 5 --seed-start 1 --jobs 2
--out /tmp/ms2-repro.jsonl --threshold 0 --log-dir /tmp/ms2-logs
--best-out /tmp/ms2-best.json --emit-best /tmp/ms2-best.hbits`

Result: exit 0. All 5 seeds ok, WNS 9140 ps, TNS 0, every seed.
BEST seed 1 (tie-break: lowest seed). Per-seed logs `seed-1.log`
through `seed-5.log` written; `seed-1.log` reads
`status=ok wns_ps=9140 tns_ps=0 endpoints=4 r2r_ps=360 iob_ps=860
pathfinder_iters=1 imux_routes=10 imux_skip=0 overused=0`.
Best-out JSON carries reproduce
`helion impl examples/counter.sv --part HL10T-C32-1 --seed 1`.
Emit-best wrote `/tmp/ms2-best.hbits`, 185 bytes.

Reproduce check: `helion impl examples/counter.sv --part
HL10T-C32-1 --seed 1` reports `hang_diag timing WNS_PS=9140
TNS_PS=0 endpoints=4 r2r_ps=360 iob_ps=860`, identical to the
sweep record. Seed 0 gives the same 9140. The counter is 4 LUTFFs;
the seed shuffle has no timing effect at this size, so a flat
sweep is expected here, not a defect.

Note: the 9140 measured here differs from the 9640 "gold WNS on
counter" comment in `compile_design_xdc`. The sweep, `helion
impl`, seed 0, and seed 1 all agree on 9140, so the sweep is
consistent with the current compile path; the 9640 comment looks
stale. Not chased here.

### Run 2: resume of run 1 (same command again)

Result: exit 0. Header `(5 already done, 0 to run)`,
`multiseed: nothing to run`, same BEST seed 1, JSONL still
5 lines. Resume skips finished seeds and appends nothing.

### Runs 3-4: --retry-failed, live

Checkpoint seeded by hand: seed 1 ok (WNS 9140), seed 2 fail
(reason "boom"). Without the flag (seeds 1..4): `(2 already
done, 2 to run)`, seeds 3 and 4 ran, seed 2 stayed failed,
report `3/4 ok, 1/4 fail`. With `--retry-failed`: `(3 already
done, 1 to run)`, seed 2 re-ran to ok, a second seed-2 record
was appended, report `4/4 ok, 0/4 fail`. Last record wins.

### Run 5: --part xcku3p fails loud (GAP-F7, live)

Command: `helion multiseed examples/counter.sv --part xcku3p
--seeds 2`. Result: exit 1, `multiseed front end: HAD xcku3p:
read .../devices/helion/parts/xcku3p.toml: No such file or
directory (os error 2)`. No seed ran. The 100-seed xcku3p flow
stays blocked on the device model (GAP-F7) plus pack/place/route/
STA on that part (GAP-F2/F3/F4/F5). Nothing invented.

### Run 6: garbage numeric flag fails loud

Command: `--seeds abc`. Result: exit 1, `multiseed: --seeds
must be a whole number, got "abc"`. No sweep ran.

### Run 7: 10-seed sweep, blinky, HL10T

Command: `./target/debug/helion multiseed examples/blinky.sv
--part HL10T-C32-1 --seeds 10 --seed-start 1 --jobs 2
--out /tmp/ms2-blinky.jsonl --threshold 0`. Result: exit 0,
10/10 ok, every seed WNS 9140, TNS 0, BEST seed 1. Same flat
sweep as the counter: tiny designs show no seed spread.

### What is proven vs what is not

- PROVEN on the generic part (HL10T): pack-once sweep,
  per-seed place/route/STA, per-seed logs, JSONL checkpoint and
  resume (including retry-failed), deterministic best-seed
  selection with the documented tie-breaks, best-seed artifacts
  plus a reproduce command that returns the same WNS, fail-loud
  on unknown part and on garbage flags.
- NOT PROVEN: the GOAL.md "10-seed, WNS 8460 to 8820" claim
  still has no run log; both designs run here sweep flat at
  9140, so that range is not reproduced and no number from it
  is reused. Seed spread needs a bigger design than blinky or
  counter; none was run here.
- NOT PROVEN: the 100-seed flow on xcku3p. Blocked on GAP-F7
  (device model) plus GAP-F2/F3/F4/F5, as recorded above.

## Verification by the second replacement (2026-11-19, branch tip 5b338d37)

Read the driver from first principles against `compile_design_xdc`
before running anything. All 7 defects from the list above are
confirmed fixed in the code at 5b338d37:

1. Pack once: `multiseed_front` packs once and shares the `Packed`
   across seeds; per-seed work is place/route/DRC/STA only.
2. Per-seed logs: `--log-dir` writes `seed-<n>.log` per seed.
3. Mutex poison: `file.lock().unwrap_or_else(|e| e.into_inner())`.
4. Last record wins: `dedup_results` inserts into a HashMap in file
   order, so the later record overwrites.
5. Control chars: `escape_json_value` replaces every char below
   0x20 with a space (lossy but keeps the JSONL line intact).
6. Best-seed artifacts: `--best-out` and `--emit-best` both wired.
7. Checked seed range: `seed_range` uses `checked_add`.

Equivalence check, `run_one_seed` vs `compile_design_xdc` at the
same seed, weight 0.0, default XDC (verified by reading both):
apply_xdc once in the front end, same `Device::load_part`, same
`pack` plus the shared `trim_iobs_to_budget`, same
`place_with_guide` with `timing_weight: 0.0` and the same timing
guide, same `RouteOpts` budget (24 iters over 2000 LUTFFs, else 8),
same `check_routed` plus DRC fail, same default 10 ns clock, same
`report_timing_routed_xdc`. Bitgen is skipped per seed and runs
only for the best seed; bitgen cannot affect timing, so the skip is
exact. The reproduce command in `--best-out` (`helion impl ...
--seed N`) uses weight 0.0 too (`cmd_impl` calls `compile_sv` with
0.0), so it reproduces the sweep numbers. The
`pack_once_matches_full_compile` test asserts WNS/TNS/endpoints
equality against `compile_design` for seeds 0 and 7.

Seed mechanism re-verified: `seeded_order` in
`crates/helion-place/src/lib.rs` is SplitMix64 plus Fisher-Yates,
seed 0 is the identity order, deterministic per seed.

New defects found by this re-read, fixed in commit d2db932a
(all with regression tests in `mod multiseed_tests`):

8. `parse_seed` searched the whole line, so a `"seed": N` inside a
   fail reason could invent a phantom record; `parse_results_text`
   read status and slack numbers from the whole line, so reason text
   could fake an ok status or a WNS. Fixed: seed parse is anchored
   to the line start, and status/WNS/TNS are read only from the head
   before `"reason":`.
9. Numeric flags (`--seeds`, `--seed-start`, `--jobs`,
   `--threshold`) silently fell back to defaults on garbage input.
   Fixed: `parse_multiseed_args` fails loud on a present but
   unparseable number; defaults still apply when the flag is absent.
10. Resume skipped recorded failures forever, so a transient
    failure (OOM, killed run) permanently lost that seed. Fixed:
    `--retry-failed` re-runs failed seeds; ok seeds never rerun.
    Resume now parses the checkpoint with the same parser as the
    final report (last record wins); a torn line from a killed run
    parses to no seed and reruns.
11. The header's "already done" count counted seeds outside this
    sweep's range. Fixed: it now counts only in-range skipped seeds.
12. The `RouteOpts` budget formula was duplicated in
    `compile_design_xdc` and `run_one_seed`. Fixed: one shared
    `route_opts_for` helper, used by both, with a boundary test.
13. One em dash in the `--emit-best` refusal message, left by
    5b338d37. Fixed to a comma. (Other pre-existing em dashes in
    `main.rs` strings and `batch_dashboard.rs` HTML are outside this
    driver and untouched.)

Known limitation, not fixed: the file mutex is per-process, so two
concurrent runs sharing one out file can interleave lines. A torn
line is skipped on resume and the seed reruns, so the failure mode
is a wasted rerun, never a wrong best seed. Do not run two sweeps
into the same out file at once.

## FLAG: two multiseed worktrees, one with uncommitted work

The orchestrator record conflates them; the on-disk state is:

- This branch, `fm/ultra-multiseed`, worktree
  `~/workspace/helion-ultra-multiseed`, tip d2db932a (after the
  fixes above), clean. This is the master-based helion-cli driver.
- `fm/ultra-bitgen-multiseed`, worktree
  `~/workspace/helion-ultra-bitgen-multiseed`, base 07d0d7df, with
  UNCOMMITTED work: new `crates/helion-ultrascale/src/multiseed.rs`
  (423 lines, the UltraScale-crate port: pack once, place plus
  preliminary STA per seed, route blocked, bitgen not implemented),
  plus diffs to `crates/helion-cli/src/main.rs` (33 lines,
  `helion multiseed` wired to `ultra::run_multiseed`),
  `crates/helion-cli/src/ultra.rs` (199 lines) and
  `crates/helion-ultrascale/src/lib.rs` (5 lines). Files last
  modified 14:18-15:17 UTC Nov 19. Not touched here: it is another
  worker's uncommitted work on another branch, and the orchestrator
  decides what happens to it. The port still carries the old forms
  of defects 3, 4, 5, 7, 8, 9, 10, 11 above (mutex unwrap,
  first-record-wins dedup, unescaped control chars, unchecked seed
  range, unanchored parse, silent flag defaults, no retry-failed,
  out-of-range done count) and has no per-seed logs or best-seed
  artifacts; whoever picks it up should apply the same hardening.
  The two JSONL shapes stay mutually readable: the port adds a
  `route_status` key its parser defaults to `"unknown"` when
  missing, and this driver ignores keys it does not know.
- The predecessor's `cargo test -j2` (pid 54046, started 15:17 UTC,
  log `/tmp/ms-test-ultra.log`) ran in the session-18 worktree,
  not this one. It finished with all suites passing: 162 tests,
  0 failed (lib 91 including 9 multiseed port tests, helion-bitgen
  bin 7 in 1777 s, plus 13, 9, 2, 3, 3, 2, 1, 22, 4, 2, 3, 0
  across the remaining targets). This branch's own suite then ran
  green too: `cargo test -j1 -p helion-cli --offline --bin helion`,
  16 passed, 0 failed (see the runs section above).
