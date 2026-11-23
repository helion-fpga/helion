# INT_X2+ set_int_columns lane report

Lane branch: fm/ultra-bitgen-intcols, code tip f3b8d52b (impl commits
adba0fc2 and f3b8d52b on intintf base 7c9f4894). No merges into
fm/ultra-bitstream, fm/zedboard-merge, or master. No critic dispatched
by this lane; Session 40 is the main agent's to dispatch.

## 1. Outcome

The gap is closed at the device level: the CLI flow device for xczu3eg
now resolves every INT tile per the tilegrid-derived INT column table,
INT_X2+ included. Before this lane the flow loaded the device with no
table attached, so INT tiles resolved by a provisional rule that agreed
with the table only for INT_X1 and mispredicted every INT_X2+ column.
The fix wires the table into `load_ultra_device`, the single production
loader the flow uses, mirroring the helion-bitgen binary exactly.

What the lane does not deliver: no flow-emitted design routes through
INT_X2+ today (section 3). The proof that INT_X2+ resolves correctly
is the unit/integration tests plus the flip test, not a flow design.
Driving a design through INT_X2+ needs a router/packer lane and is
carried, with the evidence, in section 8.

Suites: helion-ultrascale 441 passed / 0 failed / 1 ignored,
helion-cli 73 + 3 new = 76/0/1 (release), US-09 checkers 95 OK.
Stash proof both sides. Verify-bit PASS for blinky and counter on the
flow regression.

## 2. Baseline: the misprediction, measured

Baseline report: ~/workspace/intcols-logs/baseline/BASELINE_REPORT.md.
Probe (temp Rust test, run then deleted): for every (row, X) entry in
`xczu3eg_int_columns.json`, resolve `INT_X{x}Y{row*60}` via
`BitgenDevice::tile_base_far` with the table attached vs the plain
`DeviceFile` exactly as the CLI loaded it.

- 99 entries: 6 agree, 93 mismatch, 0 provisional-unresolved,
  0 table-unresolved. Agree: X=0 (col 1) and X=1 (col 4) in all 3
  rows (FARs row 0: 0x100, 0x400; row 1: 0x40100, 0x40400; row 2:
  0x80100, 0x80400). Mismatch: every X=2..32 in all 3 rows. Row 0
  in full is in `~/workspace/intcols-logs/baseline/mispredictions.txt`
  (X2 table col 7 FAR 0x700 vs provisional 0x400, through X32 table
  col 99 FAR 0x6300 vs provisional 0x3e00). Rows 1 and 2 repeat row 0
  with 0x40000 / 0x80000 added. The provisional rule lags by one INT
  column step and repeats columns in pairs. Row-share check (FAR at
  Y=r*60+59 equals FAR at Y=r*60): 0 bad.
- Table vs tilegrid: `verify_xczu3eg.py` section 10 matches the
  tilegrid on 5940/5940 INT tiles, bad=0; table 99 entries,
  conflicts=0, X range 0..32. Scratch regeneration with
  `--write-int-columns` to a scratch path (vendored table untouched)
  is byte-identical to the vendored table. Full script 37/40, the 3
  FAILs pre-existing part.json std-column ones, unrelated to
  section 10.
- Flow today (release CLI from the base, tileconn env as the counter
  lane used): blinky exit 0, 4 PIPs, all INT_X1Y0, .bit md5
  d82b70c8e29ce72f28e08c94f3a16d16 (identical to the lane value);
  counter exit 0, 54 PIPs, all INT_X1Y0 (inside the 51-56 run-to-run
  range). Hand-FASM proof: PIP
  `INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14` on INT_X2Y0 (single
  segbit, frame offset 17 bit 20) lands at FAR 0x411 (provisional
  base 0x400 + 17), not 0x711 (table base 0x700 + 17).
- Loader audit: `load_ultra_device`
  (`crates/helion-cli/src/ultra.rs:129`) is the only production
  loader the flow uses for xczu3eg (callers: `emit_ultra_bitstream`,
  `parse_ultra_bitstream`, `fasm_set_bits_equal`, reached via
  `run_impl` from `helion impl`); it attached no table and
  `set_int_columns` was never called in helion-cli. The only other
  production loader of `xczu3eg.json` is the helion-bitgen binary
  `load_device`, which already wires both tables.

## 3. The fix

One file changed: `crates/helion-cli/src/ultra.rs`. In
`load_ultra_device`, for `"xczu3eg"`, attach both tables mirroring
`helion-bitgen.rs` `load_device` exactly (same calls, same fail-loud
messages):

- `IntColumnTable::from_path(data_dir.join("xczu3eg_int_columns.json"))`
  then `file.set_int_columns(table)`; load failure
  `cannot load INT column table {}: {e}`; attach failure
  `INT column table does not match xczu3eg.json: {e}`.
- `IntfColumnTable::from_path(data_dir.join("xczu3eg_intf_columns.json"))`
  then `file.set_intf_columns(intf_table)`; same message shapes.
  The INTF table goes in with the INT table because the flow device
  must match the bitgen binary device (the ultrascale test helper
  `load_cli_device` already assumes both attached); without it
  INT_INTF/HDIO tiles do not resolve in the flow at all.

The provisional rule itself is untouched, `xcku3p` is untouched (a new
test asserts it loads with neither table), and the vendored tables are
untouched. The stale `load_ultra_device` doc comment now states the
tables are attached and INT_X2+ resolves per the tilegrid table.

Coordinator doc fix (commit f3b8d52b, doc-only, same file): the
blinky/counter flow tests pinned INT_X1Y0-only PIPs with the rationale
that the provisional rule is proven only for INT_X1. That rationale
contradicted the lane's own change, so the coordinator corrected the
`load_ultra_device` summary line, the blinky doc parenthetical, and
both "outside proven INT_X1" assertion messages to the current truth:
INT_X2+ resolves per the table; only the pinned flow output is still
INT_X1-only. No behavior change; the 3 new tests were re-run on the
final code and pass.

## 4. Tests (all in `crates/helion-cli/src/ultra.rs` tests)

- `xczu3eg_flow_device_has_both_column_tables`: flow device has
  both tables (99 INT entries, 15 INTF entries); xcku3p has neither;
  `INT_INTF_R_PCIE4_X9Y120` resolves to `0x81D00`;
  `HDIO_BOT_RIGHT_X9Y0` resolves to `0x1E00` (row 0 X9 col 30).
- `xczu3eg_flow_device_int_columns_match_table_all_99`: 18 anchors
  (X0/X1/X2/X10/X20/X32 in all 3 rows, FARs from
  `mispredictions.txt`) plus a loop over all 99 vendored table
  entries, each compared against
  `Far(ClbIoClk, row, col, 0).to_u32()` for tile
  `INT_X{x}Y{row*60}`.
- `xczu3eg_int_x2_pip_emits_at_table_far_not_provisional`: flip
  test repeating the baseline hand-FASM probe through
  `emit_ultra_bitstream`: the INT_X2Y0 PIP puts bit 20 of words[0]
  at exactly FAR `0x711` (table base 0x700 + 17), not `0x411`.

## 5. End to end: the flow cannot be made to route through INT_X2+

Seven designs tried, all with
`HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp` and
`HELION_TILECONN_PART=xczu3eg-sbva484-1-e`, release CLI built from
this branch. Every flow FASM that exists uses only INT_X1Y0; the
router builds a single INT tile (`wiredb tiles=1` in every log) and
saturates it.

| design | outcome |
|---|---|
| counter8 (8-bit, 2 slices) | ROUTE OveruseTimeout at tile (1,0), seeds 0-3 all fail |
| counter16 (16-bit) | PACK fail: X-pin repair, no free compatible letter |
| chain16 (16-stage XOR pipe) | PACK fail: X-pin repair |
| shift32 (32-bit shift reg) | PACK fail: no slice with compatible control set |
| counters2x4 (two chained 4-bit) | ROUTE OveruseTimeout at tile (1,0) |
| counter5 (5-bit, 1 slice) | ROUTE OveruseTimeout at tile (1,0), 3 retries identical |
| tiny2x2 (two chained 2-bit) | exit 0, but packs into ONE slice: 54 PIPs, all INT_X1Y0 |

The pattern is consistent: any design past the 4-bit counter's net
count fails at ROUTE OveruseTimeout on the single INT_X1Y0 tile
(even the 1-slice 5-bit counter), designs that pack cleanly into
more slices fail at PACK (X-pin repair / control set), and designs
that do route pack into a single slice. Forcing placement spread is
not available: the placer takes the first free logic sites in (Y, X)
order and the seed only reorders slices. So no flow-emitted design
exercises INT_X2+ today; the proof stands on the unit/integration
tests in section 4 plus the blinky/counter regression below. Driving
a design through INT_X2+ needs a router/packer fix, a separate lane.

## 6. Blinky / counter regression + verify-bit

Release CLI built from this branch. Flow runs (tileconn env as above):

- blinky: exit 0, 4 PIPs, all INT_X1Y0, 5,982,532 bytes, .bit md5
  `d82b70c8e29ce72f28e08c94f3a16d16`, identical to the baseline
  value.
- counter: exit 0, 56 PIPs (inside the 51-56 run-to-run range), all
  INT_X1Y0, 5,982,532 bytes.

Verify-bit, decoder built from this branch via
`US09V_DECODER_BIN` override (N34), absolute `--workdir`,
`--top` (N37), fixture RTL, 400 cycles: blinky PASS, decode 81
FASM lines exit 0, route 4 PIPs / 4 nets, cosim 800 comparisons,
0 mismatches; counter PASS, decode 139 FASM lines exit 0, route 56
PIPs / 10 nets, cosim 800 comparisons, 0 mismatches. Logs in
`~/workspace/intcols-logs/impl/` (`verify-blinky.log`,
`verify-counter.log`).

## 7. Suites and stash proofs

Suites on the impl branch, run by the lane, serially, -j1:

- helion-ultrascale debug: 441 passed / 0 failed / 1 ignored
  (log `suite-ultrascale.log`; recount from the log: 441 passed,
  0 FAILED lines). Matches baseline 441/0/1; no new ultrascale
  tests added.
- helion-cli release: 76 passed / 0 failed / 1 ignored
  (log `suite-cli-release.log`; recount: 76 passed, 0 FAILED
  lines). Baseline 73/0/1 plus the 3 new tests. Run with the
  tileconn env unset (the baseline method). Note: the plain debug
  CLI run gets SIGTERM-killed inside `corpus_1000` at ~23.6 min;
  release is the counter-lane method.
- US-09 checkers: 95 tests OK in 108.4 s
  (log `checkers-us09.log`). Matches baseline 95 OK. iverilog
  12.0 at /usr/bin/iverilog; no apt install needed.
- Coordinator re-run on final code (after the doc fix f3b8d52b):
  the 3 new tests pass
  (`cargo test -j1 -p helion-cli --release --bin helion`,
  filtered runs: 2 passed / 0 failed and 1 passed / 0 failed).

Stash proof both sides (logs `stash-proof-fail.log`,
`stash-proof-pass.log`, diff saved as `fix.patch`):

- Fix stashed, tests kept: `cargo test -j1 -p helion-cli --bin
  helion xczu3eg_` exits 101, 1 passed / 3 failed: all 3 new
  tests FAILED, the 1 pass is the pre-existing
  `xczu3eg_device_loads` test.
- Restored: 4 passed / 0 failed. Restored diff is byte-identical
  to `fix.patch`.

## 8. Incidents

- OOM kills from running suites in parallel: the first ultrascale
  debug run was SIGKILLed in the helion-bitgen bin tests and the
  first checkers run was killed (exit 137) because both ran at
  once under -j2. Reran serially at -j1 with nothing else running;
  all suites then passed. Do not run cargo suites and the checkers
  concurrently on this VM.
- Stale /tmp pollution: the first CLI suite run had
  `HELION_TILECONN_DB_ROOT` set, so the
  `blinky/counter_stops_at_route_without_pin_wire_map` tests routed
  successfully and wrote
  `/tmp/helion-us09-flow-{blinky,counter}/*.xcku3p.bit`. The next
  run (env unset) then failed those two tests at
  `stops_at_route_without_pin_wire_map`, "bitstream file exists
  despite the ROUTE failure": the flow did fail at ROUTE as
  expected, but the test asserts the file absent without deleting
  it first. Deleted the stale dirs and reran; the suite then
  passed 76/0/1. Pre-existing test fragility (fixed /tmp paths,
  no cleanup), not a code failure.

## 9. Known gaps and carried items

- Flow cannot route through INT_X2+ (section 5): needs a
  router/packer lane. The INT_X2+ proof is the unit/integration
  tests plus the flip test, not a flow design. The blinky/counter
  flow tests still assert every PIP is on INT_X1Y0, which remains
  true of what the flow emits today.
- N34 stands: `verify.py` still defaults `US09V_DECODER_BIN` to
  the stale pre-N7 decoder; every verify-bit PASS here used the
  branch-built decoder via the env override.
- Stale /tmp pollution (section 8): the
  `stops_at_route_without_pin_wire_map` tests assert a file absent
  without deleting it first; a prior env-set run leaves stale
  `.bit` files that fail the next env-unset run. Test fragility,
  fix is deleting the file first in the test.
- Carried, unchanged from the INT_INTF lane: IOB placement (still
  blocked on B1, needs a Vivado-built INT_INTF-routed design);
  INT_INTF tile model for DSP/BRAM routing; known-INIT Vivado
  datum; documented xczu3eg timing; remaining 54,907 non-band
  unexplained bits; N16 parse-side open; N9 block types 2+
  untested; N35/N36 router nondeterminism (acceptance pins verify
  outcomes, not .bit md5); N37 report omits required --top in
  verify command; N38 CLI suite counts a skipped flow test as
  passed; flags A-CTRL and A-F7F8 still unresolved, need a CTO
  ruling.

## 10. Files changed (impl branch, 1 file)

- `crates/helion-cli/src/ultra.rs`: adba0fc2 (+144/-11, wiring
  plus 3 tests); f3b8d52b (+8/-6, coordinator doc fix for the
  stale INT_X1-only claims). No other files changed. No vendored
  DB file edited.
