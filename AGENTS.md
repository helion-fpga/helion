# Project agent memory

This file is the project's committed home for project-intrinsic agent knowledge: build, test, release, architecture, and sharp-edge notes that should travel with the code.

- Add durable project-specific notes here as they are discovered through real work.

## Maintaining this file

Keep this file for knowledge useful to almost every future agent session in this project.
Do not repeat what the codebase already shows; point to the authoritative file or command instead.
Prefer rewriting or pruning existing entries over appending new ones.
When updating this file, preserve this bar for all agents and keep entries concise.

## Helion IDE chrome

`helion-ide` is three canvases (Editor / Device / Timing) plus a 6-item activity rail and More ⋯ for the other `WorkspaceTab`s. Routing lives in `crates/helion-gui/src/chrome.rs` (`pane_for_workspace`) and `crates/helion-gui/src/surface.rs` (`central_pane` / `ChromeDriver`). The binary paint path must follow `central_pane`. More destinations never fall through to Timing. Open HDL uses `open_hdl_dialog` (rfd). Linux rfd/xdg-portal is pinned to zbus 5.13.2 / zvariant 5.9.2 so rustc 1.85 CI can compile (zbus ≥5.14 needs 1.87). Implement/synth/place/route/sim_run run on a `helion-engine` thread (`spawn_job`); do not call them inside an egui frame. Debug: `HELION_UI_LOG`, `HELION_DEBUG_OVERLAY=1`. `egui::Context::set_debug_on_hover` is `#[cfg(debug_assertions)]` only — never call it on a release paint path. CI `release helion-ide` is `cargo check --release -p helion-gui --bin helion-ide`. Launch shots: `HELION_OPEN`, `HELION_FLOW=implement|synth|sim`, `HELION_WORKSPACE=<tab label>`.

## Toasts

`bin/helion-ide/toasts.rs`: small auto-dismissing notices, bottom-right above the status bar. Wall-clock expiry (5s) via `Instant` + `request_repaint_after` — never frame counts (the UI is reactive; frame-based timers linger while idle). Max 360px wide with wrapping, max 4 stacked, deduped. Job *progress* lives in the status bar; toasts are completions/errors only. No "Started" toasts.

## In-app updater

`crates/helion-update` (pure logic) + `bin/helion-ide/update_ui.rs` (UI).
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

## macOS release signing

The `.app` is ad-hoc signed (`codesign --deep --force -s -`, in `scripts/build-macos-app.sh` + verified in `release.yml`), not Apple-notarized. Without the ad-hoc signature, Sequoia-era Gatekeeper reports the download as "damaged". First launch still needs the quarantine cleared once: `xattr -cr /Applications/Helion.app` (documented in README Quickstart). Full notarization needs a paid Apple Developer ID — not set up.

## 7-series bitstream assembly

`FrameStore` must cover **every valid frame from `part.json`** (`FrameStore::for_part`, built on `PartInfo::frame_addresses`), never the union of `tilegrid.json` bit ranges. The FDRI stream has no per-frame address, so a missing frame shifts every later frame and the bitstream still reports `done=1`. The tilegrid union is 434 frames short on xc7a35t and 2,194 on xc7z020; the old symptom was a BUFG written into an unrelated CLB frame.

Clock nets are real pip routes, not free: route the spine (IOI/HCLK_CMT → BUFG_REBUF → HROW → INT CLK) and emit the per-stage active/enable companion bits (`clock_companions` in `emit.rs`). **Every stage has a gating bit and a mux bit; the mux alone leaves the branch muted.** Missing so far, all found only by diffing against a working Vivado bitstream: `HCLK_L.ENABLE_BUFFER` (the HCLK_L arm did not exist, so left-edge clocks -- i.e. the ZedBoard GCLK -- died at HROW), `CLK_HROW.BUFHCE.BUFHCE_X1Y<n>.IN_USE` (index `n` pairs with `CLK_HROW_CK_MUX_OUT_{L,R}<n>`), and input-pad `PULLTYPE.NONE`/`IN_TERM.NONE` (inputs default to a pulldown; the clock pin is an input).

`clock_companions` is a hand-written table and has been wrong twice; two of the four stages we model were missing bits. Prefer deriving companions from the DB the way pips are derived. Check with `python3 scripts/clkdiff.py <ours.bit>` -- it diffs the whole spine against `~/CDA4253_A1/blinky.bit` and prints features MISSING vs Vivado. Note prjxray silently drops unmatched FASM lines, so a clean assemble proves self-consistency only.

IOB relative site names are top-down: the upper IOB in a tile is `IOB_Y0` (see `XDevice::site_rel`). An output IOB's fabric entry is NOT `site.O` -- that is the pad side of the OBUF, and routing to it lands on a neighbouring tile's IMUX. Use `RouteQuery::iob_output_sink`, which resolves the OLOGIC's `D1` input. For LED0 (T22) the entry is `INT_R_X73Y46.IMUX34`, one tile north of the pad's own tile.
