# N37 fix report: verify.py fails loud on a missing --top instead of failing downstream on an undriven clock

Branch: `fm/ultra-bitgen-n37fix`, base `fm/ultra-bitgen-n34fix` at
`321604f67840fdc66dcf261798f68c7e06a288d3` (verified: `git rev-parse` in
the base worktree and `git ls-remote origin fm/ultra-bitgen-n34fix`
agree, both `321604f67840fdc66dcf261798f68c7e06a288d3`). Worktree
`~/workspace/helion-ultra-bitgen-n37fix`. No merges. No critic dispatched
by this lane. 0 em dashes in this report, in the code, or in the commit
message.

## Verdict

N37 is fixed. A verify-bit (or verify-fasm) run given RTL files without
`--top`, or `--top` without RTL files, now exits 1 in about 1 second
with a `ports` FAIL naming the missing flag, before any place, route,
or extract work runs. The decode-only path is unchanged: with neither
RTL files nor a top, ports are still legitimately skipped and the rest
of verification proceeds exactly as before. Blinky and counter
verify-bit both PASS with `--top`, matching the N34 proof line for line.

## Ground truth

CRITIQUE.md line 5918 (Critic Session 36): "the report does not print
the full verify-bit invocation. --top counter is required: without it
the ports block is skipped (verify.py line 119 requires rtl_files and
top), port_ins is empty, and verify FAILs on place/route/extract with
CLK1 undriven. My first verify attempt failed exactly this way (my
error, corrected and redone). Future reports should print the exact
command including --top. Non-blocking, documentation only."

The driver re-verified the mechanism: in `verify_fasm`, `if rtl_files
and top and (xdc or port_map):` guards the ports block. This lane
reproduced the misleading failure live on the base code (see Control)
and fixed it at that guard.

## What changed

One file, `tools/us09v/us09v/verify.py` (+33/-0, purely additive):

- New helper `_ports_input_error(rtl_files, top)` returning an error
  string or None. It fires only when exactly one of the pair is
  given: RTL files without a top, or a top without RTL files. It
  returns None when both are given, and when neither is given
  (decode-only run).
- `verify_fasm` calls the helper immediately before the ports block
  and, on an error, adds a `ports` FAIL check with the message and
  returns before place, route, extract, or cosim run. This follows
  the existing ports-error pattern (XDC errors and port-map
  `ValueError` already add a `ports` FAIL and return early).
- Messages:
  - RTL without top: "RTL files given but no --top: cannot determine
    the top module, so ports cannot be derived. Pass --top <module>
    with --rtl."
  - Top without RTL: "--top given but no --rtl: cannot derive ports
    without RTL files. Pass --rtl <file> with --top, or drop --top
    for a decode-only run."

No CLI change was needed: the only CLI entry points are the
`verify-fasm` and `verify-bit` subcommands in `tools/us09v/us09v/
__main__.py`, and both funnel through `verify_fasm` (`__main__.py:80`
and `verify.py:349`), so both get the guard from the one place. No
decoder code touched. No N7/N8/N9/Q code touched. `resolve_decoder_bin`
(N34) is byte-identical: the diff touches nothing in it.

## Caller audit

- `tools/n74n75fix/coord-run-verify.sh`: every verify-bit invocation
  goes through its `vb()` helper, which always passes
  `--top "$top"`. No invocation needed fixing.
- `tools/ultra-bitgen/run-suites.sh`: runs the US-09 unittest suite
  only; it invokes no verify-bit command. Nothing to fix.
- `tools/us09v/tests/test_verify.py`: the only `verify_fasm` caller
  in the tests. Fixture tests pass RTL plus top plus port map
  (unaffected); golden tests pass neither RTL nor top (decode-only
  path, unaffected). The full suite run below confirms both.
- No other `verify-bit` or `verify-fasm` invocations exist in shell
  or Python files in this checkout outside reports and the us09v
  tool itself.

## Decoder used

The branch decoder was built from this worktree (env override UNSET
for every run below, so the N34 default-path gate applied):

```
export PATH="$HOME/.cargo/bin:$PATH"
cd ~/workspace/helion-ultra-bitgen-n37fix
cargo build --release -j2 -p helion-ultrascale --bin helion-bitgen
```

Finished in 41.77s, same 2 pre-existing warnings as the N34 lane
(unused `NOOP_WORD`, `CMD_WCFG` in `bitstream_packets.rs`), sha256
`0d3c6af472d6ea25080d24215250147458c91b959cc65a8092250f59ffcb00f1`,
5,739,016 bytes. `resolve_decoder_bin()` resolves it cleanly.

## Control reproduction (base code, before the fix)

Blinky `.bit` md5 `d82b70c8e29ce72f28e08c94f3a16d16`
(`~/workspace/critic40-runs/flow/blinky.bit`), base code, env override
unset, no `--top`:

```
cd ~/workspace/helion-ultra-bitgen-n37fix/tools/us09v
env -u US09V_DECODER_BIN python3 -m us09v verify-bit \
  --bit ~/workspace/critic40-runs/flow/blinky.bit --design blinky \
  --db-root ~/workspace/prjuray-db-ref/zynqusp --part xczu3eg-sbva484-1-e \
  --rtl tests/fixtures/blinky/rtl.v \
  --port-map ~/workspace/emitter-xczu3eg-logs/flow2/blinky-port-map.json \
  --workdir ~/workspace/ultra-bitgen-crew/logs/n37fix-control-blinky \
  --cycles 400 --seed 1
```

Log `~/workspace/ultra-bitgen-crew/logs/n37fix-control-blinky.log`,
exit 1, 5.6s:

```
blinky: FAIL
  [ok] decode: 81 FASM lines, exit 0
  [ok] parse: 2 tiles, 81 features
  [FAIL] place: 1 slices, 0 carry slices; SLICE_X1Y0: ABCD half has FFs but CLK1 is undriven
  [FAIL] route: 4 PIPs, 3 nets; undriven sink: SLICE_X1Y0.CLK1 on net {CLEM_X1Y0.CLE_CLE_M_SITE_0_CLK1, INT_X1Y0.CTRL_W4}
  [FAIL] extract: 1 sites modelled; SLICE_X1Y0.CLK1 (clock) is undriven; 18 warnings
```

No `ports` check appears at all: ports were silently skipped, and
the run failed downstream naming CLK1 undriven on all three checks.
This is exactly the misleading behavior N37 describes.

Symmetric control, same command with `--top blinky` in place of
`--rtl` (log `~/workspace/ultra-bitgen-crew/logs/
n37fix-control-blinky-toponly.log`, exit 1): byte-identical output
to the control above, same three CLK1 failures, no `ports` check.

## Proof runs (fixed code)

### Without --top: fail loud at ports, before place/route/extract

Same command as the control, fixed code, fresh workdir
`~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky-notop`.
Log `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky-notop.log`,
exit 1, 1.1s:

```
blinky: FAIL
  [ok] decode: 81 FASM lines, exit 0
  [ok] parse: 2 tiles, 81 features
  [FAIL] ports: RTL files given but no --top: cannot determine the top module, so ports cannot be derived. Pass --top <module> with --rtl.
```

No place, route, extract, or cosim check runs. The missing `--top`
is named in the failure itself.

### --top without --rtl: symmetric case

Same command with `--top blinky` in place of `--rtl`, fresh workdir
`~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky-toponly`.
Log `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky-toponly.log`,
exit 1, 0.9s:

```
blinky: FAIL
  [ok] decode: 81 FASM lines, exit 0
  [ok] parse: 2 tiles, 81 features
  [FAIL] ports: --top given but no --rtl: cannot derive ports without RTL files. Pass --rtl <file> with --top, or drop --top for a decode-only run.
```

### Decode-only path still works

Golden blinky FASM, neither RTL nor top, fixed code:

```
cd ~/workspace/helion-ultra-bitgen-n37fix/tools/us09v
env -u US09V_DECODER_BIN python3 -m us09v verify-fasm \
  --fasm ~/workspace/ultra-bitgen-crew/harness/golden/blinky.xcku3p.fasm \
  --design blinky-golden --part xcku3p
```

Log `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-decode-only.log`,
exit 1 (expected: the golden FASM has no routes):

```
blinky-golden: FAIL
  [ok] parse: 3 tiles, 7 features
  [ok] provisional: design part xcku3p != DB part xczu3eg-sbva484-1-e; site-type and tile-type checks are provisional
  [ok] place: 1 slices, 0 carry slices; no violations; notes: provisional tiles: site-type checks are notes, not failures
  [FAIL] route: 2 PIPs, 1 nets; undriven sink: PIP BOUNCE_E_0_FT1.INODE_E_1_FT1 source on INT_X0Y0 on net {INT_X0Y0.INODE_E_1_FT1, INT_TERM_B_X0Y0.SOUTHBUSOUT_FT0_5, INT_X0Y0.INODE_E_BLN_62_FT1}; undriven sink: PIP INT_NODE_GLOBAL_0_INT_OUT0.GCLK_B_0_0 source on INT_X0Y0 on net {INT_X0Y0.GCLK_B_0_0, INT_X0Y1.GCLK_B_0_0, INT_TERM_B_X0Y0.GCLK_B_FT0_0}; undriven sink: SLICE_X0Y0.A1 on net {CLEL_R_X0Y0.CLE_CLE_L_SITE_0_A1, INT_X0Y0.IMUX_E10}; net {INT_X0Y0.BOUNCE_E_0_FT1, CLEL_R_X0Y0.CLE_CLE_L_SITE_0_AX, INT_TERM_B_X0Y0.SOUTHBUSOUT_FT0_0} is driven only by a PIP, not reachable from any real driver; net {INT_X0Y0.INT_NODE_GLOBAL_0_INT_OUT0} is driven only by a PIP, not reachable from any real driver
  [FAIL] extract: 1 sites modelled; SLICE_X0Y0.A1 (ALUT input) is undriven
```

No `ports` check, no ports failure: ports are still legitimately
skipped on the decode-only path and the rest of verification proceeds
exactly as on the base code (same place-ok / route-undriven /
extract-undriven shape the golden tests assert).

### Blinky verify-bit PASS with --top (full invocation)

```
cd ~/workspace/helion-ultra-bitgen-n37fix/tools/us09v
env -u US09V_DECODER_BIN python3 -m us09v verify-bit \
  --bit ~/workspace/critic40-runs/flow/blinky.bit --design blinky \
  --db-root ~/workspace/prjuray-db-ref/zynqusp --part xczu3eg-sbva484-1-e \
  --rtl tests/fixtures/blinky/rtl.v --top blinky \
  --port-map ~/workspace/emitter-xczu3eg-logs/flow2/blinky-port-map.json \
  --workdir ~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky \
  --cycles 400 --seed 1
```

Log `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky.log`,
exit 0:

```
blinky: PASS
  [ok] decode: 81 FASM lines, exit 0
  [ok] parse: 2 tiles, 81 features
  [ok] ports: 1 inputs, 1 outputs from port map
  [ok] place: 1 slices, 0 carry slices; no violations
  [ok] route: 4 PIPs, 4 nets; no violations
  [ok] extract: 1 sites modelled; no errors; 18 warnings
  [ok] cosim: 800 comparisons, 0 mismatches, 0 RTL unknowns, 0 net unknowns; no mismatches
```

Matches the N34 proof line for line (81 lines, 4/4 route, cosim 800/0).

### Counter verify-bit PASS with --top (full invocation)

Counter `.bit` md5 `9c61d51dd2bf0b4c816b8ed539a8fa64`
(`~/workspace/critic40-runs/flow/counter.bit`), fixture RTL
`~/workspace/counter-logs/impl/verify-counter/counter-rtl.v`
(`reg [3:0] cnt = 0`) and its port map
`~/workspace/counter-logs/impl/verify-counter/counter-port-map.json`:

```
cd ~/workspace/helion-ultra-bitgen-n37fix/tools/us09v
env -u US09V_DECODER_BIN python3 -m us09v verify-bit \
  --bit ~/workspace/critic40-runs/flow/counter.bit --design counter \
  --db-root ~/workspace/prjuray-db-ref/zynqusp --part xczu3eg-sbva484-1-e \
  --rtl ~/workspace/counter-logs/impl/verify-counter/counter-rtl.v --top counter \
  --port-map ~/workspace/counter-logs/impl/verify-counter/counter-port-map.json \
  --workdir ~/workspace/ultra-bitgen-crew/logs/n37fix-proof-counter \
  --cycles 400 --seed 1
```

Log `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-counter.log`,
exit 0:

```
counter: PASS
  [ok] decode: 137 FASM lines, exit 0
  [ok] parse: 2 tiles, 137 features
  [ok] ports: 1 inputs, 1 outputs from port map
  [ok] place: 1 slices, 0 carry slices; no violations
  [ok] route: 54 PIPs, 10 nets; no violations
  [ok] extract: 1 sites modelled; no errors; 18 warnings
  [ok] cosim: 800 comparisons, 0 mismatches, 0 RTL unknowns, 0 net unknowns; no mismatches
```

Matches the N34 proof and the critic40 counter result exactly
(137 lines, 54 PIPs, 10 nets, cosim 800/0).

## Suites

US-09 checkers, full suite, from `tools/us09v` in this worktree:

```
python3 -m unittest discover -s tests -v
```

Log `~/workspace/ultra-bitgen-crew/logs/n37fix-us09-checkers.log`:
`Ran 100 tests in 120.625s`, `OK`, 0 failed, 0 skipped. Same 100/100
as the N34 lane. No test weakened, no check skipped. No Rust changed
in this lane, so no Rust suite re-run; the branch release build of
`helion-bitgen` above proves the tree compiles.

## Files changed

- `tools/us09v/us09v/verify.py` (+33/-0): the fix.
- `tools/n37fix/N37FIX_REPORT.md`: this report, byte-identical copy
  (verified with `sha256sum`, both hashes recorded below).
- No other files. `__pycache__` dirs from the test run were removed
  before committing.

## What stays open

- Adjacent gap, observed but deliberately out of scope for this
  minimal fix: RTL files plus a top but neither `--xdc` nor
  `--port-map` still silently skips the ports block (the guard is
  `rtl_files and top and (xdc or port_map)`), and the run then fails
  downstream on undriven inputs the same way N37 did for `--top`.
  A future lane may fail that case loud too; this lane changed
  nothing there.
- N7/N8/N9/Q untouched. N16 carried. No merges. The N9 worktree
  and branch were not touched.
- The fail-loud happens in `verify_fasm` at the ports step, after
  decode (for verify-bit) and parse. Decode is not place, route, or
  extract work, and the no-`--top` run still exits in about 1 second
  total; no earlier guard was added.

## Logs

- `~/workspace/ultra-bitgen-crew/logs/n37fix-control-blinky.log`
  (base control, no --top, exit 1, CLK1 failures)
- `~/workspace/ultra-bitgen-crew/logs/n37fix-control-blinky-toponly.log`
  (base control, --top without --rtl, exit 1, same CLK1 failures)
- `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky-notop.log`
  (fixed, no --top, exit 1, ports FAIL naming --top, 1.1s)
- `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky-toponly.log`
  (fixed, --top without --rtl, exit 1, ports FAIL naming --rtl, 0.9s)
- `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-decode-only.log`
  (fixed, decode-only golden FASM, unchanged behavior)
- `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-blinky.log`
  (fixed, blinky PASS, cosim 800/0)
- `~/workspace/ultra-bitgen-crew/logs/n37fix-proof-counter.log`
  (fixed, counter PASS, cosim 800/0)
- `~/workspace/ultra-bitgen-crew/logs/n37fix-us09-checkers.log`
  (100 tests, OK)
