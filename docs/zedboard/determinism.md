# ZedBoard zedbit cross-platform determinism check

Date: 2026-11-19. Branch: `fm/zedboard-determinism`, based on `fm/zedboard-merge`
at `7ec3192`. Worktree: `~/workspace/helion-zedboard-determinism`.

## Result

STOPPED: no bitstreams were produced, so no md5 comparison against the Mac
reference was possible. Both zedbit runs fail at device load because the local
Project X-Ray database tree is missing `zynq7/mapping/` (see below). This is a
finding about the local database tree, not about zedbit determinism: nothing
here says the Linux and Mac bitstreams would differ.

## Machine

- `uname -sm`: `Linux x86_64`
- Mac (arm64) reference md5s: blinky `d622e9831c0fa76faa5cbaec1aea2267`,
  pico `08ca33a6ff702841ca8851cf20b023c4`
- Linux md5s: none produced (no `blinky.bit`, no `pico.bit`)
- Match vs Mac reference: not determinable, no Linux bitstreams exist

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
zedbit: unknown part xc7z020clg484-1: not in the Project X-Ray database at /home/hatch/workspace/xilinx-dbsrc (families artix7; 0 parts, e.g. )
```

```
zedbit synth: v2 front end, 1464 LUTs, 516 flops, 6 LUT levels
zedbit: synthesized in 0.9s; loading the device
zedbit: unknown part xc7z020clg484-1: not in the Project X-Ray database at /home/hatch/workspace/xilinx-dbsrc (families artix7; 0 parts, e.g. )
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

## What would unblock the check

Restore `zynq7/mapping/` (at minimum `parts.yaml` and `devices.yaml`) in
`~/workspace/xilinx-dbsrc/` from the upstream database, then re-run the two
commands above and record the md5s. The database tree is shared state
outside this branch, so it was deliberately not modified here; the restore
needs whoever owns that tree to do it (or to point `HELION_XILINX_DB` at a
complete checkout). After the restore, also confirm `xc7z020clg400-1/` is
present or accept the `required_features.fasm` warning path.

## Notes

- A first build waiter was started at 17:07 UTC and lost in a daemon
  restart that wiped `/tmp` (the old `/tmp/zedbit-build.log` held one line
  from 16:33 UTC from the previous worker's waiter, which never built).
  The build above was relaunched after the restart, once `ps` showed no
  `cargo`/`rustc`, with logs kept under `~/workspace/` so they survive a
  restart.
- No files under the hands-off paths were touched.
