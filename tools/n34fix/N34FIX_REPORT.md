# N34 fix report: verify.py no longer silently measures a stale decoder

Branch: `fm/ultra-bitgen-n34fix`, base `fm/ultra-bitgen-n16fix` at
`827b05865ef2ec94a5a6757cf20e2e5e923ee857` (verified: `git ls-remote origin`
and `git rev-parse` in the worktree agree). Worktree
`~/workspace/helion-ultra-bitgen-n34fix`. No merges. No critic dispatched
by this lane. 0 em dashes in this report, in the code, or in the commit
messages.

## Verdict

N34 is fixed. The default decoder is now the release binary of the
checkout running verify.py, and verify.py refuses to run a default
decoder that is missing or older than the decoder sources in this
checkout, naming the rebuild command. Blinky and counter verify-bit
both PASS with the env override unset on a branch-built decoder, where
the same commands FAILED on decode under the stale default before the
fix. The US-09 checkers suite passes 100/100.

## Fix chosen: fail loud, not auto-build

The critic's direction allowed either a default built from the current
branch or a loud failure on a pre-N7 binary. I picked fail loud, for
three reasons. First, auto-building inside verify.py would turn a
checker into a builder: every verify-bit run could silently trigger a
multi-minute cargo build and hide build failures. Second, fail loud is
deterministic and reviewable: the error names the binary, the newest
source it predates, and the exact rebuild command. Third, the
repointed default already removes the trap N34 observed (the default
pointed at a different, old worktree's binary); the gate covers the
remaining staleness mode, a binary built before the current sources.

## What changed

One file, `tools/us09v/us09v/verify.py` (+94/-2):

- The default `DECODER_BIN` is now `<repo root>/target/release/helion-bitgen`,
  derived from `__file__`, instead of the hardcoded
  `~/workspace/helion-ultra-bitgen-decoder/target/release/helion-bitgen`.
- New `resolve_decoder_bin()`, called by `decode_bitstream()` (so the
  `decode` subcommand gets the same gate). It raises `RuntimeError`
  instead of running when the default binary is missing or older than
  the newest decoder source in this checkout
  (`crates/helion-ultrascale/src/**/*.rs`, its `Cargo.toml`, its `data/`
  files). Both errors name the rebuild command
  `cargo build --release -j2 -p helion-ultrascale --bin helion-bitgen`
  and the override option.
- An explicit `US09V_DECODER_BIN` is the caller's assertion about which
  decoder to use: it is used as-is once it exists (no staleness gate),
  and a missing override path now raises a clear `RuntimeError` naming
  the path instead of a `FileNotFoundError` traceback from subprocess.
- Import stays safe for `verify_fasm`-only use: the gate runs at decode
  time only, never at import.

No decoder code touched. No N7/N8/N9/Q code touched. No check weakened:
every verify-bit outcome below still runs the full place/route/extract/
cosim chain.

## Control reproduction (base code, before the fix)

At lane start the default-path binary did not exist at all:
`~/workspace/helion-ultra-bitgen-decoder/target/` was absent, the
binary wiped since the critic's runs. To restore the documented state
I built the stale decoder from `fm/ultra-bitgen-decoder` at `f35560a5`
in scratch worktree `~/workspace/helion-ultra-bitgen-n34fix-stale`
(detached HEAD, verified `f35560a58264e00d27e4fd613ddb81c8799d7691`):

```
cd ~/workspace/helion-ultra-bitgen-n34fix-stale
cargo build --release -j2 -p helion-ultrascale --bin helion-bitgen
```

Finished in 39.53s, 2 pre-existing warnings (unused `NOOP_WORD`,
`CMD_WCFG` in `bitstream_packets.rs`), sha256
`458916427a7e9853472c485db98ac77adf194fca2800b63ff210e27fdd0317c6`.
Copied to the default path (sha256 matches). Then, env override unset,
base verify.py, blinky `.bit` md5 `d82b70c8e29ce72f28e08c94f3a16d16`:

```
cd ~/workspace/helion-ultra-bitgen-n34fix/tools/us09v
env -u US09V_DECODER_BIN python3 -m us09v verify-bit \
  --bit ~/workspace/critic40-runs/flow/blinky.bit --design blinky \
  --db-root ~/workspace/prjuray-db-ref/zynqusp --part xczu3eg-sbva484-1-e \
  --rtl tools/us09v/tests/fixtures/blinky/rtl.v --top blinky \
  --port-map ~/workspace/emitter-xczu3eg-logs/flow2/blinky-port-map.json \
  --workdir ~/workspace/ultra-bitgen-crew/logs/n34fix-control-blinky \
  --cycles 400 --seed 1
```

Log `~/workspace/ultra-bitgen-crew/logs/n34fix-control-blinky.log`,
exit 1:

```
blinky: FAIL
  [FAIL] decode: exit 1: helion-bitgen: decode failed: 41 set bits no feature explains (listed above)
  [ok] parse: 2 tiles, 81 features
  [ok] ports: 1 inputs, 1 outputs from port map
  [ok] place: 1 slices, 0 carry slices; no violations
  [ok] route: 4 PIPs, 4 nets; no violations
  [ok] extract: 1 sites modelled; no errors; 18 warnings
  [ok] cosim: 800 comparisons, 0 mismatches, 0 RTL unknowns, 0 net unknowns; no mismatches
```

The exact 41-bit failure from CRITIQUE.md:6058 reproduces. The stale
binary alone, direct decode, also exits 1 with exactly
`41 set bits no feature explains`
(log `~/workspace/ultra-bitgen-crew/logs/n34fix-stale-decode-blinky.err`).

Counter control, same base code, env override unset, counter `.bit`
md5 `9c61d51dd2bf0b4c816b8ed539a8fa64`, RTL `examples/counter.sv`:

```
env -u US09V_DECODER_BIN python3 -m us09v verify-bit \
  --bit ~/workspace/critic40-runs/flow/counter.bit --design counter \
  --db-root ~/workspace/prjuray-db-ref/zynqusp --part xczu3eg-sbva484-1-e \
  --rtl ~/workspace/helion-ultra-bitgen-n34fix/examples/counter.sv --top counter \
  --port-map ~/workspace/emitter-xczu3eg-logs/flow2/blinky-port-map.json \
  --workdir ~/workspace/ultra-bitgen-crew/logs/n34fix-control-counter \
  --cycles 400 --seed 1
```

Log `~/workspace/ultra-bitgen-crew/logs/n34fix-control-counter.log`,
exit 1: decode FAIL, `268 set bits no feature explains`. The cosim line
in that run (`0 comparisons, 800 RTL unknowns`) is an RTL artifact:
`examples/counter.sv` leaves `cnt` uninitialized; it is not the decoder
defect, and the proof run below uses the counter-lane fixture RTL.

Counter5 cross-check, stale binary direct decode on
`~/workspace/critic41-runs/counter5-s0.bit` (md5
`9939f36f68b50e39be852d9784456fd4`): exit 1, `386 set bits no feature
explains`. The critic recorded 394 on a different emission of the same
design; the count is run-dependent (router nondeterminism, N35/N36)
and the class reproduces. Log
`~/workspace/ultra-bitgen-crew/logs/n34fix-stale-decode-counter5.err`.

## Gate unit checks (fixed code)

Before building the branch decoder, with the env override unset:

- Missing default binary: `resolve_decoder_bin()` raises
  `RuntimeError: decoder binary not built:
  /home/hatch/workspace/helion-ultra-bitgen-n34fix/target/release/helion-bitgen
  does not exist. Build it from this checkout with cargo build --release
  -j2 -p helion-ultrascale --bin helion-bitgen (run in
  /home/hatch/workspace/helion-ultra-bitgen-n34fix), or set
  US09V_DECODER_BIN to a helion-bitgen binary built from the current
  branch.`
- Backdated stale binary at the default path (copy of the `f35560a5`
  binary, `touch -d "2026-11-20 12:00:00"`): raises
  `RuntimeError: stale decoder binary: ... predates the decoder sources
  in this checkout
  (.../crates/helion-ultrascale/src/wire_db.rs is newer than the
  binary). A binary built before the N7-port decode change fails decode
  with unexplained band bits that are not real. Rebuild it ...`
- `python3 -m py_compile tools/us09v/us09v/verify.py`: ok.

## Proof runs (fixed code, env override UNSET, branch decoder)

Branch decoder built from the n34fix worktree (same cargo command as
above, 41.16s, same 2 pre-existing warnings), sha256
`36cb8081b959d74810f8bd08292087255de1d26ac4d7318c86de9d469979e8aa`,
5,739,016 bytes. `resolve_decoder_bin()` resolves it cleanly. Direct
decode of blinky `.bit`: exit 0, 81 lines.

Blinky, same command as the control except a fresh workdir
(`~/workspace/ultra-bitgen-crew/logs/n34fix-proof-blinky`):

Log `~/workspace/ultra-bitgen-crew/logs/n34fix-proof-blinky.log`,
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

Counter, same command as the proof form except the fixture RTL
(`~/workspace/counter-logs/impl/verify-counter/counter-rtl.v`,
`reg [3:0] cnt = 0`, matching the flow FF INIT 0) and its port map,
fresh workdir `~/workspace/ultra-bitgen-crew/logs/n34fix-proof-counter2`:

Log `~/workspace/ultra-bitgen-crew/logs/n34fix-proof-counter2.log`,
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

This matches the critic40 counter result exactly (137 lines, 54 PIPs,
10 nets, cosim 800/0). The fixture RTL is needed because the flow
`counter.sv` leaves `cnt` uninitialized, so plain RTL cosim reports
800 RTL unknowns and 0 comparisons (routepack report section 5, and
the lane's own first counter proof run reproduced that artifact before
switching to the fixture RTL). Acceptance pins verify outcomes, not
`.bit` md5 (N35/N36).

## Override unchanged

Same counter proof command with `US09V_DECODER_BIN` set to the stale
binary at the old default path: exit 1, decode FAIL,
`268 set bits no feature explains`, cosim 800/0 on the partial FASM.
Log `~/workspace/ultra-bitgen-crew/logs/n34fix-override-stale-counter.log`.
The override is still honored as-is; only the default changed.

## Suites

US-09 checkers, from `tools/us09v` in this worktree:

```
python3 -m unittest discover -s tests -v
```

Log `~/workspace/ultra-bitgen-crew/logs/n34fix-us09-checkers.log`:
`Ran 100 tests in 122.017s`, `OK`, 0 failed, 0 skipped. That includes
`test_p11_decode_exit_1_unexplained`, which now RUNS instead of
skipping (the default binary exists) and passes with the branch
decoder. No Rust changed in this lane, so no Rust suite re-run; the
branch release build of `helion-bitgen` above proves the tree
compiles.

## Files changed

- `tools/us09v/us09v/verify.py` (+94/-2): the fix.
- `tools/n34fix/N34FIX_REPORT.md`: this report, byte-identical copy
  (verified with `cmp`).
- No other files. `__pycache__` dirs from the test run were removed
  before committing.

## What stays open

- Residual of the mtime gate, stated not hidden: the gate catches a
  default binary older than the current decoder sources, and the
  repointed default removes the cross-worktree trap N34 observed. A
  stale binary built from older sources and then copied into this
  checkout's `target/release` after checkout would carry a fresh mtime
  and pass the gate. That mode needs a deliberate manual copy. The
  binary carries no version marker to fingerprint instead: the N7 port
  (`f9f0962a`) changed only internal decode behavior, no CLI flag and
  no version string.
- The old default path still holds the stale `f35560a5` binary as an
  inert build artifact; nothing points at it after this fix.
- N7/N8/N9/Q untouched. N16 carried. No merges.

## Logs

- `~/workspace/ultra-bitgen-crew/logs/n34fix-stale-build.log` (stale
  decoder build, 39.53s)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-stale-decode-blinky.out`
  / `.err` (stale direct decode, 41 bits)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-stale-decode-counter5.out`
  / `.err` (stale direct decode, 386 bits)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-control-blinky.log`
  (control, blinky FAIL 41 bits)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-control-counter.log`
  (control, counter FAIL 268 bits)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-branch-build.log` (branch
  decoder build, 41.16s)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-branch-decode-blinky.out`
  / `.err` (branch direct decode, exit 0)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-proof-blinky.log` (proof,
  blinky PASS)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-proof-counter.log` (proof
  with uninitialized RTL: decode ok, cosim 800 RTL unknowns)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-proof-counter2.log`
  (proof with fixture RTL, counter PASS)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-override-stale-counter.log`
  (override still honors the stale decoder, FAIL 268 bits)
- `~/workspace/ultra-bitgen-crew/logs/n34fix-us09-checkers.log`
  (100 tests OK)
