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

## macOS release signing

The `.app` is ad-hoc signed (`codesign --deep --force -s -`, in `scripts/build-macos-app.sh` + verified in `release.yml`), not Apple-notarized. Without the ad-hoc signature, Sequoia-era Gatekeeper reports the download as "damaged". First launch still needs the quarantine cleared once: `xattr -cr /Applications/Helion.app` (documented in README Quickstart). Full notarization needs a paid Apple Developer ID — not set up.
