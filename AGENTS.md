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
