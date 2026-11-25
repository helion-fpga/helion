# ZedBoard zedbit cross-platform determinism check

Date: 2026-11-19. Branch: `fm/zedboard-determinism`, based on `fm/zedboard-merge`
at `7ec3192`. Worktree: `~/workspace/helion-zedboard-determinism`.

## Result

RERUN COMPLETE 2026-11-19: both bitstreams were produced on Linux x86_64
after restoring `zynq7/mapping/` from f4pga prjxray-db master (see below).
Both md5s DIFFER from the Mac arm64 reference. That is a real finding about
cross-platform determinism, not a run failure: both zedbit runs exited 0.

- Linux blinky md5: `e13faaf255dc16c7c9463d748a6b5956` (4,045,671 bytes),
  Mac reference `d622e9831c0fa76faa5cbaec1aea2267`: MISMATCH
- Linux pico md5: `afe395dd2583f1264d81b409765cb9a4` (4,045,664 bytes),
  Mac reference `08ca33a6ff702841ca8851cf20b023c4`: MISMATCH

The first attempt (same day, before the restore) produced no bitstreams: both
runs failed at device load because the local database tree was missing
`zynq7/mapping/` (see the original log below). The restore unblocked the runs;
the mismatch verdict comes from the rerun only.

Note: the `.bit` header date is fixed (`2026/11/11 00:00:00` in both files),
so the mismatch is not a build-timestamp artifact. The cause of the
difference (placement, routing, or nondeterminism across platforms) was not
investigated in this check; that is follow-up work.

## Machine

- `uname -sm`: `Linux x86_64`
- Mac (arm64) reference md5s: blinky `d622e9831c0fa76faa5cbaec1aea2267`,
  pico `08ca33a6ff702841ca8851cf20b023c4`
- Linux md5s (rerun 2026-11-19): blinky
  `e13faaf255dc16c7c9463d748a6b5956`, pico
  `afe395dd2583f1264d81b409765cb9a4`
- Match vs Mac reference: MISMATCH on both designs

## Database revision at `~/workspace/xilinx-dbsrc/`

No revision marker exists in the tree: it is not a git checkout (`git log`
there fails with "not a git repository"), and it holds no version, revision,
README, or commit file at the top level. The revision therefore cannot be
pinned to a commit hash from local markers, and it must not be claimed to
match `0c3504f` or f4pga master.

Content evidence, checked 2026-11-19:

- `zynq7/ppips_hclk_ioi3.db` locally has 36 lines, md5
  `2a20d9e57d777a1c914245496ad5e209`. Its non-blank content lines match
  `f4pga/prjxray-db` master line for line in the same order (36 lines).
  `openXC7/prjxray-db@0c3504f` has 40 content lines in the same file, with 4
  extra `IO_PLL_CLK*_DMUX ... default` lines. Byte-level equality with f4pga
  master could not be established from the text fetch, so this is a
  content-line match, not a byte proof.
- The tree has no `spartan7/`, `kintex7/`, or `virtex7/` directories, which
  `openXC7/prjxray-db@0c3504f` adds over f4pga master.
- `crates/helion-xilinx7/DB.md` describes this tree as f4pga master as of
  2026-10-12. That claim is consistent with the file check above, but it is
  not independently verified here.

So: most consistent with f4pga master content, not with `0c3504f`, but no
commit hash can be asserted for the local tree.

## Commands run

All from the worktree `~/workspace/helion-zedboard-determinism`, with
`HELION_XILINX_DB=$HOME/workspace/xilinx-dbsrc` (the root holds `zynq7/`).

Build (started only after `ps` showed no `cargo`/`rustc` running; the
session 18 test run had finished by then):

```
cargo build --release -j1 -p helion-xilinx7 --example zedbit
```

Build log: `~/workspace/zedbit-build.log`. Result: exit 0,
`Finished release profile [optimized] target(s) in 7m 03s`, binary at
`target/release/examples/zedbit`.

Input files verified present before running: `examples/zedboard_blinky.sv`,
`examples/zedboard.xdc`, `examples/zedboard_pico_soc.sv`,
`third_party/picorv32/picorv32.v`, `examples/zedboard_pico_soc.xdc`.

Runs (log: `~/workspace/zedbit-runs.log`):

```
./target/release/examples/zedbit examples/zedboard_blinky.sv examples/zedboard.xdc blinky.bit zedboard_blinky --ignore-timing
./target/release/examples/zedbit examples/zedboard_pico_soc.sv,third_party/picorv32/picorv32.v examples/zedboard_pico_soc.xdc pico.bit zedboard_pico_soc --ignore-timing
```

Both runs exit 1 within a second, after synthesis, at device load:

```
zedbit synth: v2 front end, 27 LUTs, 27 flops, 1 LUT levels
zedbit: synthesized in 0.0s; loading the device
zedbit: unknown part xc7z020clg484-1: not in the Project X-Ray database at ~/workspace/xilinx-dbsrc (families artix7; 0 parts, e.g. )
```

```
zedbit synth: v2 front end, 1464 LUTs, 516 flops, 6 LUT levels
zedbit: synthesized in 0.9s; loading the device
zedbit: unknown part xc7z020clg484-1: not in the Project X-Ray database at ~/workspace/xilinx-dbsrc (families artix7; 0 parts, e.g. )
```

## Root cause

The loader (`crates/helion-xilinx7/src/parts.rs`, `families()`) only counts a
family directory when `<family>/mapping/parts.yaml` exists, and `resolve()`
reads the part from that file plus `<family>/mapping/devices.yaml` for the
fabric. The local tree has `artix7/mapping/` (`devices.yaml`,
`parts.yaml`) but no `zynq7/mapping/` at all, so `zynq7` is invisible to the
loader: families are `artix7` only, 0 parts, and `xc7z020clg484-1` is unknown.

The `zynq7/` data itself is largely present: `xc7z020/tilegrid.json`,
`xc7z020/tileconn.json`, `xc7z020clg484-1/part.json`,
`xc7z020clg484-1/package_pins.csv`, `xc7z020clg484-1/part.yaml`, plus the
`mask_*`, `ppips_*`, `segbits_*`, `tile_type_*`, and `site_type_*` files.

Upstream `f4pga/prjxray-db` master does ship `zynq7/mapping/parts.yaml`
(it lists `xc7z020clg484-1` with device `xc7z020`, package `clg484`,
speedgrade `1`) and `zynq7/mapping/devices.yaml` (`xc7z020` fabric
`xc7z020`). The local tree is therefore an incomplete copy of the upstream
tree, missing at least `zynq7/mapping/`.

One further gap, not fatal on its own: the local tree has no
`xc7z020clg400-1/` part directory. `crates/helion-xilinx7/src/device.rs`
falls back to another part on the same fabric for `required_features.fasm`
and only warns when none exists, so this alone would not stop a run, but a
restored tree should include it for parity with upstream.

Note: `DB.md` (committed at `7ec3192`) reports byte-identical bitstreams
from both databases with the same zedbit. That comparison must have used a
complete database checkout elsewhere, not this `~/workspace/xilinx-dbsrc`
tree in its current state, because this tree cannot resolve the ZedBoard
part at all. That is an observation, not a verified explanation.

## Rerun with restored zynq7/mapping (2026-11-19, second attempt)

Decision: use the current f4pga prjxray-db master files. The md5 difference
against the user's stated md5s is expected: the local mapping files come from
a different source with a different layout (quoted keys, no package or
speedgrade fields). Both map `xc7z020clg484-1` to device `xc7z020` and fabric
`xc7z020`, and Helion reads only `parts.yaml` and `devices.yaml`, so
`resources.yaml` is not needed and was not created.

Fetch and verification (before copying anything in):

```
curl -fsSL -o parts.yaml https://raw.githubusercontent.com/f4pga/prjxray-db/master/zynq7/mapping/parts.yaml
curl -fsSL -o devices.yaml https://raw.githubusercontent.com/f4pga/prjxray-db/master/zynq7/mapping/devices.yaml
md5sum parts.yaml devices.yaml
```

- `parts.yaml`: 852 bytes, md5 `1d0dbc937f4e4cb8c3085dc1bb699d2f`, matches
  expected
- `devices.yaml`: 89 bytes, md5 `40a5419f2cc464556139bed6dad4b274`, matches
  expected

Copied into `~/workspace/xilinx-dbsrc/zynq7/mapping/` (directory created);
md5s re-verified after copying. No `resources.yaml` created.

Rerun, same two commands as above, same binary
(`target/release/examples/zedbit`, no rebuild), same
`HELION_XILINX_DB=$HOME/workspace/xilinx-dbsrc`:

Blinky, exit 0:

```
zedbit: part xc7z020clg484-1
zedbit: packed 27 LUTs, 27 flops, 7 carry cells, 0 RAMB36 + 0 RAMB18, 0 DSP48, 0 LUT RAMs (0 LUT RAM LUTs), 9 pads; placing
zedbit: placed in 4.2s
zedbit: routing
zedbit: routed in 18.2s
zedbit timing: worst clock period 10.000 ns, worst slack 6.130 ns (0 of 27 endpoints failing), est. fmax 258 MHz on that clock
zedbit: examples/zedboard_blinky.sv -> blinky.bit (570 FASM lines, 1312 features, cells {"Carry4": 7, "Hff": 27, "IobOut": 8, "Lut6": 27}, 27 luts / 27 ffs / 9 iobs packed)
```

Pico, exit 0 (about 185 s, mostly routing):

```
zedbit timing: worst clock period 10.000 ns, worst slack 8.467 ns (0 of 1258 endpoints failing), est. fmax 652 MHz on that clock
zedbit: examples/zedboard_pico_soc.sv,third_party/picorv32/picorv32.v -> pico.bit (37975 FASM lines, 61515 features, cells {"BlackBox": 11, "Carry4": 74, "Hff": 516, "IobOut": 8, "Lut6": 1464}, 1464 luts / 516 ffs / 15 iobs packed)
```

md5s, from the worktree:

```
md5sum blinky.bit pico.bit
e13faaf255dc16c7c9463d748a6b5956  blinky.bit
afe395dd2583f1264d81b409765cb9a4  pico.bit
```

Verdict: MISMATCH on both designs against the Mac arm64 reference. The
`.bit` header date is fixed (`2026/11/11 00:00:00` in both files, per
`strings`), so the difference is not a build timestamp. `x7v verify` was not
run on these bitstreams as part of this check; per the project AGENTS.md a
bitstream is not done until `x7v verify` passes, so that is a separate step
if these bitstreams are to be used further.

## What would unblock the check

Done: `zynq7/mapping/` (`parts.yaml` and `devices.yaml`) was restored in
`~/workspace/xilinx-dbsrc/` from f4pga prjxray-db master on 2026-11-19, and
the two commands were re-run with the md5s recorded above. The database tree
is shared state outside this branch; the restore was done on the user's
explicit order for this check.

Remaining gap, not fatal on its own: the local tree still has no
`xc7z020clg400-1/` part directory. `crates/helion-xilinx7/src/device.rs`
falls back to another part on the same fabric for `required_features.fasm`
and only warns when none exists, so this alone would not stop a run, but a
restored tree should include it for parity with upstream. The rerun above
succeeded without it.

Open follow-up: investigate why the Linux and Mac bitstreams differ (placement,
routing, or nondeterminism across platforms), and re-check md5s after any fix.

## Notes

- A first build waiter was started at 17:07 UTC and lost in a daemon
  restart that wiped `/tmp` (the old `/tmp/zedbit-build.log` held one line
  from 16:33 UTC from the previous worker's waiter, which never built).
  The build above was relaunched after the restart, once `ps` showed no
  `cargo`/`rustc`, with logs kept under `~/workspace/` so they survive a
  restart.
- No files under the hands-off paths were touched.

## Resolution (2026-11-19, third run)

Root cause confirmed: the local database had no `required_features.fasm`
for `xc7z020clg484-1` (upstream f4pga master does not ship one for that
part either) and no `xc7z020clg400-1/` part directory at all, so zedbit
warned and left the three always-on bits out of the FASM. The Linux FASM
differed from the Mac FASM only by those 3 lines
(`CFG_CENTER_MID_X123Y84.ALWAYS_ON_PROP1` to `3`); placement and routing
were identical. The diagnosis was confirmed by rebuilding on the
Mac with a database shaped like the Linux one and reproducing the Linux
md5s byte for byte.

Fix: restored `zynq7/xc7z020clg400-1/` from f4pga prjxray-db master
(`package_pins.csv`, `part.json`, `part.yaml`,
`required_features.fasm` carrying exactly those 3 lines). The loader falls
back to another part on the same fabric for `required_features.fasm`, so
the clg400-1 file supplies the always-on bits for clg484-1 builds.

Rerun, same two commands, same binary (real captured stdout this time,
not kept in the repo), both exit 0:

- blinky md5 `d622e9831c0fa76faa5cbaec1aea2267`: MATCHES Mac
- pico md5 `08ca33a6ff702841ca8851cf20b023c4`: MATCHES Mac
- Both FASM files now contain the 3 ALWAYS_ON lines.

Verdict: Helion is deterministic across Linux x86_64 and Mac arm64 for
these two designs. T3 closed. Follow-up logged as gap G81:
make zedbit and x7v fail loudly when `required_features.fasm` is missing
instead of warning and silently dropping always-on bits.
