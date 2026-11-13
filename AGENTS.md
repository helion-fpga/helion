# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- Add durable project-specific notes here as they are discovered through real work.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.

## Rune IDE (`helion-rune`)

`crates/helion-rune` builds the shipped `helion-ide` binary (Helion.app, release tarballs, `helion gui`). Rune itself is vendored at one upstream commit in `third_party/rune` (its own workspace, excluded from ours): never edit it in place; change Rune upstream and run `scripts/sync-rune.sh <rune-checkout> <commit>`. Windowless modes: `helion-ide --headless <file>` (gold: counter.sv → `WNS_PS=9640`), `--source script.tcl`, `--stdin`, `--version`, `--doctor`; all go through `tcl::eval_batch`. `init()` is the in-memory demo project used by every test and headless `--snap/--scene`; only an interactive launch uses `boot()` (Start page or `HELION_OPEN`/path arg, prefs via `rune::prefs`, `RUNE_PREFS_DIR` redirects them). Tests must never write outside the temp dir: `pickers::default_projects_dir()` is temp only under `cfg(test)`, so integration tests set the wizard location explicitly. In `Runtime` tests, `rt.frame(1.0/60.0)` repeats one instant, so animations (toast entrance) never settle; advance time or `invalidate()` after mutating `rt.state`, and move the pointer before a tap. UI rule: no echo-only controls; a control does real work or renders disabled with a reason. Port status and remaining gaps: `crates/helion-rune/SWEEP.md`.

## In-app updater

`crates/helion-update` holds the logic. Its UI lived in the egui IDE
(`crates/helion-gui`), which master removed; the Rune IDE does not call it yet,
so the rest of this section describes the intended behaviour.
Check runs once on fresh session startup, on a background thread; no periodic
checks while running. `HELION_NO_UPDATE_CHECK=1` skips it. GitHub Releases via
`self_update` 0.42 (pinned: 1.x needs Rust 1.88, workspace MSRV is 1.85);
SHA256SUMS verification is manual with `sha2` (0.42 has no checksums feature).
In-place install only when the install dir is user-writable (probed with a
tempfile, not permission bits); otherwise download-only to `~/Downloads`.
macOS `.app` installs swap the whole bundle (replacing one binary would break
the ad-hoc signature); Developer ID-signed bundles refuse in-place and fall
back to download-only. Restart saves the editor, spawns the new binary with
the same args, then closes. Menu: Help -> "Check for updates…".

## 7-series flow (ZedBoard, Arty A7, Basys3, Nexys A7)

`crates/helion-xilinx7/examples/zedbit.rs` builds a `.bit` from HDL and an XDC
for the ZedBoard's `xc7z020clg484-1`, or `--board`/`--part` (any part the X-Ray
database describes; `helion_xilinx7::parts`, board pins in `boards/`); `tools/x7v` decodes any 7-series bitstream with the
Project X-Ray database and co-simulates it against the RTL (`python3 -m x7v
verify|flow`, see its README). A bitstream is not done until `x7v verify`
passes on it, and timing is a separate gate (`zedbit` refuses a design that
misses its clock). `zedbit --front=v2` uses the v2 SystemVerilog front end;
`--front=zb` uses the front ends frozen in `crates/zb` (read its README before
touching them: they are copies, not to be edited), which are the only ones
that infer carry chains. Database: `HELION_XILINX_DB` (directory holding
`zynq7/`, `artix7/`; default `~/.cache/helion/prjxray-db`), `X7V_DB_ROOT` for
x7v. `tools/x7v/tests/boards.sh` builds and verifies a counter and PicoRV32 per
board. Never flash a board without the user watching.

## macOS release signing

The `.app` is ad-hoc signed (`codesign --deep --force -s -`, in `scripts/build-macos-app.sh` + verified in `release.yml`), not Apple-notarized. Without the ad-hoc signature, Sequoia-era Gatekeeper reports the download as "damaged". First launch still needs the quarantine cleared once: `xattr -cr /Applications/Helion.app` (documented in README Quickstart). Full notarization needs a paid Apple Developer ID — not set up.
