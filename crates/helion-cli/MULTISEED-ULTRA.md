# Multiseed timing closure, UltraScale+ port: verification record (GAP-F6)

Branch: `fm/ultra-bitgen-multiseed`, base `07d0d7df`, worktree
`~/workspace/helion-ultra-bitgen-multiseed`. No merges, no pushes.
Vivado license state UNKNOWN; nothing here runs Vivado or claims
Tier 2.

## What the port is

Port of the multiseed proof on fm/ultra-multiseed (e1c2e47) into
`crates/helion-ultrascale/src/multiseed.rs` plus the driver in
`crates/helion-cli/src/ultra.rs` (`helion multiseed <design>
--part xcku3p --seeds N --seed-start S --jobs J --out FILE
--threshold T --log-dir DIR --best-out FILE --emit-best FILE
--retry-failed`):

- Synth once, pack once (seed independent), then place plus legality
  plus preliminary STA per seed on a rayon pool sized by `--jobs`.
- Each seed appends one JSONL record `{seed, wns_ps, tns_ps,
  runtime_s, status, reason, route_status}` under a file mutex,
  flushed per record, so a killed run keeps finished seeds.
- Resume: seeds already in the JSONL are skipped; `--retry-failed`
  re-runs failed seeds, ok seeds never rerun.
- Best seed: highest `wns_ps` among `status == "ok"` and
  `wns_ps >= threshold`; ties break on lowest `tns_ps`, then
  lowest seed. Deterministic over the JSONL on disk.
- Per-seed logs: `--log-dir` writes `seed-<n>.log` per seed.
- Best-seed artifact: `--best-out` writes the best seed, its
  numbers, `route_status`, and the exact `helion impl <design>
  --part <part> --seed N` reproduce command.
- Seed determinism comes from the same SplitMix64 seeded
  Fisher-Yates in `place::seeded_order`, threaded through
  `UltraPlaceOpts.seed`. Same seed gives the same placement,
  hence the same STA result.

## Hardening applied here (same as the HL10T driver, d2db932a)

1. `parse_seed` anchored to the line start; status, WNS, and TNS
   read only from the head before `"reason":`; `route_status` only
   from the tail after the reason value's closing quote; status
   normalizes to ok/fail. Reason text cannot fake a record, a
   status, a slack number, or a route status.
2. `to_jsonl` escapes every control character below 0x20 to a
   space via shared `escape_json_value` (was: backslash, quote,
   newline only).
3. File mutex poisoning is recovered instead of panicking the
   sweep.
4. Dedup keeps the LAST record per seed (was: first wins).
5. `seed_range` uses checked arithmetic and fails loud on
   overflow or zero seeds.
6. Numeric flags (`--seeds`, `--seed-start`, `--jobs`,
   `--threshold`) fail loud on garbage input instead of silently
   falling back to defaults.
7. Header "already done" count covers only seeds in this sweep's
   range (was: every seed ever recorded in the file).
8. Route budget has one home: the impl flow takes
   `UltraRouteOpts::default()` (max_iters 50) instead of
   restating the literal. The multiseed path routes nothing yet
   (route blocked), so there is no second budget to share; the
   shared helper lands with the routing graph.
9. No em dashes in any new or changed line. (Pre-existing em
   dashes in `main.rs` hw/bitstream strings are outside this
   driver and untouched, same as on the HL10T branch.)

Known limitation, not fixed: the file mutex is per-process, so
two concurrent runs sharing one out file can interleave lines.
A line torn before the seed field parses to no seed and reruns;
a line torn after the seed parses as a failed record for that
seed, skipped on resume unless `--retry-failed` re-runs it. The
failure mode is a wasted rerun, never a wrong best seed. Do not
run two sweeps into the same out file at once.

## What is NOT on this branch

- ROUTE is blocked for every seed: the fabric WireDb is empty (no
  routing graph yet; routing-graph worker owns it), so `route_ultra`
  is never reached. Each record carries `route_status: "blocked"`
  and the summary report says so. A seed with place plus STA green
  is `status: "ok"` for place-plus-STA only, never for the full
  flow.
- BITGEN is not implemented on this base. No bitstream is produced
  by any seed. `--emit-best` is accepted and refused loud.
- Timing numbers are ESTIMATES until the xcku3p delay DB exists
  (GAP-F5). STA uses `timing_db::preliminary_xcvu3p` (ballpark
  delays: LUT 120ps, FF ckq 150ps, setup 80ps, one INT hop 180ps).
  Every report that shows WNS says so.
- The proof's 10-seed WNS range 8460 to 8820 is recorded in
  US-09-end-to-end-flow.md only; no run artifact for it exists in
  either repo, so it is not reproduced here and no number from it
  is reused. The runs in this crate produce their own numbers.

## JSONL mutual readability (port vs proof)

The two shapes stay mutually readable: the port adds a
`route_status` key the proof driver ignores (its parser reads
only seed, status, wns_ps, tns_ps from the line head), and the
port parser defaults a missing `route_status` to `"unknown"`
(proof-era lines), covered by the
`jsonl_parse_proof_line_without_route_status` test.

## Proven vs blocked (status at this commit)

- PROVEN (mechanism, xcku3p, blinky): pack-once sweep, per-seed
  place plus legality plus preliminary STA, per-seed logs, JSONL
  checkpoint and resume (including retry-failed and in-range
  done counting), deterministic best-seed selection with the
  documented tie-breaks, best-seed artifact plus a reproduce
  command that returns the same WNS, fail-loud on garbage flags
  and on `--emit-best`. Reproduction runs below.
- NOT PROVEN: the 100-seed flow on xcku3p. Blocked on the device
  model (GAP-F7, no xcku3p.toml) plus GAP-F2/F3/F4/F5 (pack/place/
  route/STA on that part). This port must not invent a device
  model, delays, or routing to work around that.
- NOT PROVEN: any seed spread. Blinky is 1 LUT plus 1 FF; both
  seeds measure the same WNS 9650 ps, so a flat sweep is expected
  here, not a defect. Seed spread needs a bigger design; none was
  run here.

## Verification runs

(Commands and outputs appended here as they are run. Nothing in
this file is claimed without a shown run.)

Binary: `target/debug/helion`, built 2026-11-19 with
`cargo build -j1 -p helion-cli --bin helion --offline`.
Tests: `cargo test -j1 -p helion-ultrascale --offline --lib`:
100 passed, 0 failed (91 pre-existing plus 9 new: 8 multiseed
hardening tests, 1 route-budget default test).
Clippy: plain `cargo clippy -j1 -p helion-ultrascale --offline
--all-targets` exits 101 on the pre-existing `helion-sv` deny at
`crates/helion-sv/src/lib.rs:15059` (same block as on the HL10T
branch, out of GAP-F6 scope, not fixed here), so it never reaches
this crate. With `RUSTFLAGS="--cap-lints warn"` it completes,
exit 0, with 0 warnings in the port files after two fixes found
by that run (needless lifetime on `select_best_seed`, useless
`format!` in a test).
`cargo check -j1 -p helion-cli --offline`: clean except the 2
pre-existing unused-import warnings in `main.rs` (`place_with`,
`route_with`, also present at base).

### Run 1: 2-seed sweep, blinky, xcku3p (2026-11-19 ~17:52 UTC)

Command: `./target/debug/helion multiseed examples/blinky.sv
--part xcku3p --seeds 2 --seed-start 1 --jobs 1
--out /tmp/ms-port-run1.jsonl --threshold 0
--log-dir /tmp/ms-port-logs --best-out /tmp/ms-port-best.json`

Result: exit 0. All 2 seeds ok, WNS 9650 ps, TNS 0, every seed.
BEST seed 1 (tie-break: lowest seed). Per-seed logs `seed-1.log`
and `seed-2.log` written; `seed-1.log` reads
`status=ok wns_ps=9650 tns_ps=0 endpoints=1 r2r_ps=270 iob_ps=0
placed_slices=1`. Best-out JSON carries reproduce
`helion impl examples/blinky.sv --part xcku3p --seed 1`.

Reproduce check: `helion impl examples/blinky.sv --part xcku3p
--seed 1` prints `hang_diag timing WNS_PS=9650 TNS_PS=0
endpoints=1 r2r_ps=270 iob_ps=0`, identical to the sweep record,
then stops at the route gate (`ULTRA ROUTE: WireDb is empty`)
with exit 1. The reproduce command returns the same WNS; the
route gate is the documented blocker, not a sweep defect.

### Run 2: resume of run 1 (same command again)

Result: exit 0. Header `(2 already done, 0 to run)`,
`multiseed: nothing to run`, same BEST seed 1, JSONL still
2 lines. Resume skips finished seeds and appends nothing.

### Runs 3-4: --retry-failed and in-range done count, live

Checkpoint seeded by hand: seed 1 ok (WNS 9650), seed 2 fail
(reason "boom"), seed 99 ok (outside the 1..4 range). Without
the flag (seeds 1..4): `(2 already done, 2 to run)`, seeds 3
and 4 ran, seed 2 stayed failed, report `4/5 ok, 1/5 fail`
(the 5th record is the out-of-range seed 99; the summary covers
all records on disk, same as the proof driver). With
`--retry-failed`: `(3 already done, 1 to run)`, seed 2 re-ran
to ok, a second seed-2 record was appended, report `5/5 ok,
0/5 fail`. Last record wins. The out-of-range seed 99 was
never counted in "already done" in either run.

### Run 5: garbage numeric flag fails loud

Command: `--seeds abc`. Result: exit 1, `multiseed: --seeds
must be a whole number, got "abc"`. No sweep ran, no JSONL
created.

### Run 6: --emit-best refused loud

Command: `--emit-best /tmp/ms-port-best.hbits` on the finished
run-1 checkpoint. Result: exit 1, `multiseed: --emit-best
refused: UltraScale+ bitgen is not implemented on this base
(route blocked, no bitstream exists to emit)`. No bitstream
file created.
