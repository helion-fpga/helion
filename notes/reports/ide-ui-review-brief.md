# Helion IDE UI Review Brief

**Branch:** `wip/ide-integration` (latest)
**For:** Codex (or any agent doing UI review + improvements)
**Standard:** Saksham's directive, Oct 11-13

---

## 1. Design Standard (non-negotiable)

The current IDE surfaces were judged mediocre. Redesign must be grounded in
real, popular apps' surfaces, not taste-driven iteration in a vacuum. Study:

- **Vivado's device view and reports** (table density, how space is filled)
- **Quartus** (panel chrome, typography scale)
- **VS Code's chrome** (activity rail, panel zones, spacing rhythm)
- **GTKWave** (waveform viewer density, signal list layout)
- **One premium non-EDA app** (your pick, for spacing rhythm and polish)

Concrete patterns to lift: spacing rhythm, table density, typography scale,
how empty space is filled vs left, zone systems (what belongs where and why).

Apple HIG / GNOME HIG / Material may inform, but the EDA references come first.

---

## 2. Spacing System (enforced)

- Use the `SPACE_XS / SPACE_SM / SPACE_MD / SPACE_LG / SPACE_XL` scale.
  No magic-number spacings. (83 were eliminated Oct 13; do not regress.)
- Every nested `ScrollArea` inside another scrollable region MUST use
  `data_scroll(id)` (`bin/helion-ide.rs`): it pins
  `auto_shrink([false, true])` + `max_height(TABLE_MAX_HEIGHT)` and needs a
  unique `id_salt`. Salt-less areas leak scroll offsets between views.
- Page flow content (wizard, dashboard sections) must NOT get an inner scroll;
  the outer page owns scrolling.

---

## 3. Known Defects (all photo-filed by Saksham, all must be fixed)

### 3a. Device view screen (Oct 13 photo)
- [ ] `???` placeholder text in the clock-regions table. No placeholder
      strings may ever render. Regression rule: a test scans every rendered
      view for `???` and other placeholder strings.
- [ ] Zoom In / Out / Fit duplicated across two rows. Deduplicate.
- [ ] 15 buttons in two rows with zero grouping. Group them with labels
      and separators following the VS Code/Vivado zone grammar.
- [ ] Inspector hint cut off mid-word. Fix layout so hints are fully visible.
- [ ] Status lines crammed together at the bottom. Apply spacing scale.

### 3b. Lying scroll indicator (Oct 13 photo)
- [ ] Minimap/scroll thumb sits at the bottom though the user has not
      scrolled there and there is nothing below.
- Fix rule: thumb position MUST equal the real scroll offset over the real
  content height at every position and every window size.
- Regression test must prove it (recompute 20x bit-identical is the
  established pattern from the power-view fix).

### 3c. Power-view mouse bug (Oct 13)
- [ ] Per-instance-random HashMap nondeterminism made hover/click targets
      unstable. Fixed with sorted iteration; keep it that way.
- Do not reintroduce iteration-order dependence in any view's hit-testing.

### 3d. Black gaps and status bar (Oct 13-14)
- [ ] Black-gap defects between panels (were screenshot-verified as fixed;
      re-verify after any layout change).
- [ ] Status-bar defects (crammed lines, misaligned items).

### 3e. Fuzzer findings (Oct 13)
- [ ] The fuzzer ended with SIGTERM and no result. Crashes found by fuzzing
      must become regression tests with replayable input sequences.
- Parallel-sergeant order stands: click every button, scroll everything,
  resize, minimize/maximize, random clicks; log every anomaly with a
  replayable sequence.

---

## 4. Audit State

- **219 controls** enumerated across 10 GUI files (Oct 14 static audit):
  zero `todo!()`, zero `unimplemented!()`, zero "not implemented" strings,
  zero empty click handlers. Every control routes to a real handler.
- This is a CODE-PATH audit only. It does NOT prove all 219 controls work
  at runtime. The runtime click-through is still open.

---

## 5. Merge Bar (Saksham's rule, Oct 13)

No IDE work merges on tests alone. A change merges only when:

1. The workflow is walkable end to end (not just "doesn't crash").
2. The agent has personally inspected the actual rendered surfaces with
   screenshots (not trusted claims, not test output).
3. Screenshots are inspected for the defect classes in section 3
   (placeholders, duplicated controls, ungrouped buttons, cut-off text,
   crammed lines, lying scroll thumbs, black gaps).

---

## 6. How to Work (for Codex)

1. Check out `wip/ide-integration` (latest).
2. Build: `cargo check --release -p helion-gui --bin helion-ide`
   (this is the CI release gate).
3. Launch headless smoke: `./target/debug/helion-ide --headless examples/blinky.sv`
   must print timing with no panic.
4. For GUI inspection: `xvfb-run` + screenshot tool, or run on a real
   display. Required env for launch shots: `HELION_OPEN`,
   `HELION_FLOW=implement|synth|sim`, `HELION_WORKSPACE=<tab label>`.
5. Debug flags: `HELION_UI_LOG=1`, `HELION_DEBUG_OVERLAY=1`.
6. Toasts: bottom-right, max 4 stacked, 360px max width, 5s wall-clock
   expiry via `Instant` + `request_repaint_after`. No "Started" toasts;
   job progress lives in the status bar.
7. Engine jobs (implement/synth/place/route/sim_run) run on a
   `helion-engine` thread via `spawn_job`. NEVER call them inside an egui
   frame.
8. `egui::Context::set_debug_on_hover` is `#[cfg(debug_assertions)]` only.
   Never on a release paint path.
9. macOS `.app` is ad-hoc signed (`scripts/build-macos-app.sh`); full
   notarization needs a paid Apple Developer ID (not set up). First launch
   needs `xattr -cr /Applications/Helion.app`.

---

## 7. Definition of Done

- Every item in section 3 is fixed and regression-tested.
- Screenshots of the fixed surfaces are attached to the report.
- `cargo check --release -p helion-gui --bin helion-ide` is green.
- Headless smoke passes.
- No new `todo!()` / `unimplemented!()` / placeholder strings.
- Spacing scale used everywhere; no magic numbers.
- The report lists: what was found, what was fixed, screenshots before/after,
  and anything that could NOT be verified (with why).

Do not claim verification that was not actually done. A browser task
claiming "verification passed" is not proof; check its own numbers.
