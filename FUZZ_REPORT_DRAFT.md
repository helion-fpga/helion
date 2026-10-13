# FUZZER Final Report: Helion IDE Bug Hunt

## Driving mechanism
**Chosen: in-process egui event fuzzing.** A test-only harness
(`crates/helion-gui/src/bin/helion-ide/fuzz.rs`) instantiates the real
`HelionIde`, then pumps frames through `egui::Context::run` with synthetic
`egui::Event`s (mouse move/click/drag, scroll wheel, window resize, key
press/release, text input, copy/cut/paste, zoom). Deterministic: seeded
splitmix64 RNG + synthetic `RawInput.time` clock (1/60s per frame). No X
server needed.

**Rejected: Xvfb + xdotool.** xdotool/ydotool/wtype do not exist and cannot be
installed (no apt/pip access for them); writing an XTEST client was
unnecessary once the in-process path worked. The in-process path is also
fully deterministic, which X input injection would not be.

To share the exact frame path with zero duplication, `eframe::App::update`'s
body was factored into `HelionIde::update_inner` (the `&mut eframe::Frame`
was ignored).

## Totals
- Frames run: ~8,500+ (8-seed long run: 6,112 frames / 22,084 events; seed-1
  deep run: 1,992 frames / 7,207 events; plus smoke tests and repro runs)
- Events generated: ~31,000+
- Unique crashes: 2 reproducible bugs (5 crash logs) + 1 flaky hang
- Regression tests added: 3 (all #[ignore]d while bugs are open)
- Invariant anomalies: 0 (after fixing the checker's bounds mirror)

## Bugs found

### 1. device_band_share clamp panic (chrome.rs:901)
Resizing to a very short window while the Device canvas shows panics:
`(after - tables_floor - clock_h - 10.0).clamp(64.0, after)` with
`after < 64`. `f32::clamp` panics when min > max.
- Repro: seed 11400714819323198485, frame 410, `EV resize 3840.0 200.0`
- Also seen at other window heights (same root cause)
- Regression test: `replay_device_band_share_short_window` (ignored)
- Crash logs: crash-b8e9fc06b1caf963.log (+ 2 duplicates)

### 2. "More Device" leaves canvas stale (surface.rs apply_more)
The "More Device" command (via palette AND via flow-nav sidebar) sets
`workspace = Device` but leaves `canvas = Editor`. The app's own
`debug_assert_eq!` in `open_more_workspace` fires: "More Device updated
workspace but painted Editor". In release builds the Device view silently
fails to appear.
- Root cause: `apply_more`'s catch-all sets `activity = Files` but does not
  sync `canvas` for canvas tabs (Device, TextEditor).
- Repro: seed 3, frame 138 (palette); seed 6, frame 515 (flow nav)
- Regression tests: `replay_more_device_palette_assertion`,
  `replay_more_device_flownav_assertion` (both ignored)
- Crash logs: crash-87c5907c652b0759.log, crash-59c2055905bbbe0e.log

### 3. Infinite loop after ~200s frame (FLAKY, UNDER INVESTIGATION)
Two occurrences: seed 1 hung after frame 577 (206s frame with PageDown+Enter
keyboard events, then 97% CPU spin); validation run hung after frame 1683
(213s frame, then spin). Both show the pattern: one ~200s frame, then the
next frame never returns. Did NOT reproduce on shorter runs (600 frames) or
on the default seed. Not yet minimized; may be timing/resource dependent.
The fuzzer now flushes its event log every 500 frames so a hang never loses
the repro prefix.

## Invariant checks (all clean)
- No NaN/inf geometry, no negative rect sizes
- Sidebar/console sizes within sane bounds
- Timing values unchanged by selection-only events
- No stuck dialogs (open/close balanced)
- No shape-count explosions
- No freezes other than bug #3

## Mechanism limitations
- Wall-clock leaks: toast expiry uses `Instant`; engine thread scheduling
  can shift which frame observes async job results. The input sequence is
  exact; timing-dependent state may vary.
- `debug_assert!`s only fire in debug (test) builds; release builds would
  silently misbehave instead of panicking for bug #2.
- The fuzzer cannot detect visual glitches (only programmatic invariants).
- File isolation: the warmup opens a scratch copy of the design
  (`<logdir>/fuzz_design_scratch.sv`), never the repo file, because the
  fuzzer types in the editor and the app auto-saves. (Without this, early
  runs corrupted `examples/blinky.sv` and `examples/hier.sv`; restored.)

## Code health

### Refactored
- **`HelionIde::update_inner` (helion-ide.rs)**: extracted the entire
  `eframe::App::update` body into a method taking `&egui::Context`. The real
  app and the headless fuzzer now share one frame path with zero duplication.
  (The `&mut eframe::Frame` was ignored by the body.)
- **`pump_frame` (fuzz.rs)**: single helper wrapping `ctx.run` in
  `catch_unwind`, replacing three copies of the panic-capture logic
  (warmup, fuzz loop, replay).

### Structural observations (for the audit)
- **Bug #1 (`device_band_share`)**: the clamp
  `(after - tables_floor - clock_h - 10.0).clamp(64.0, after)` bakes in the
  assumption `after >= 64`. A fix that just swaps the clamp args would be a
  hack; the band layout needs restructuring so small heights degrade
  gracefully (e.g., collapse bands instead of producing an impossible range).
- **Bug #2 (`apply_more`)**: the catch-all arm sets `activity` but not
  `canvas`, leaving the two out of sync for canvas tabs. The state-transition
  functions in surface.rs (`apply_more`, `apply_canvas`, `apply_activity`)
  each partially overlap; they should delegate to one canonical transition
  instead of re-implementing arms.
- No other app modules were touched; the fuzzer is test-only code.
