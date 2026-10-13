# Helion IDE Fuzz Log

Fuzzing sergeant session started: 2026-10-13 18:22 EDT
Branch: wip/ide-integration (commit 9f67c02)
Harness: in-process egui event fuzzer (`crates/helion-gui/src/bin/helion-ide/fuzz.rs`)
Method: `HELION_FUZZ_RUN=1 cargo test -p helion-gui --bin helion-ide fuzz_run`

Target: minimum 2 hours of active fuzzing across multiple seeds.

## Format
Each anomaly entry: timestamp, surface, action sequence (replayable), expected vs actual, replay seed/log.

---

## Session Log

### 18:22 - Session start
- Building test binary with current code (commit 9f67c02, includes spacing scale changes)
- Prior known crashes (from fuzz_crashes/, all marked #[ignore] as open bugs):
  1. crash-b8e9fc06b1caf963: device_band_share clamp panic on very short window (seed 11400714819323198485, frame 410)
  2. crash-87c5907c652b0759: "More Device" palette assertion, canvas=Editor mismatch (seed 3, frame 138)
  3. crash-59c2055905bbbe0e: same as #2 via flow-navigator sidebar (seed 6, frame 515)

### 18:35 - Crash replay verification (before main fuzz run)
Replayed all 3 known crash logs against current code:
- crash-b8e9fc06b1caf963: REPLAY CLEAN. The device_band_share clamp bug was fixed by prior integration work.
- crash-87c5907c652b0759: REPLAY CLEAN. The apply_more(Device) canvas sync bug was fixed by prior integration work.
- crash-59c2055905bbbe0e: NEW PANIC at frame 357 (was frame 515). Different signature:
  - Old: apply_more assertion (canvas=Editor mismatch)
  - New: `min > max, or either was NaN. min = 10.0, max = 1.0` in `minimap_viewport` (editor.rs:686)
  - Root cause: `(viewport_h / content * track).clamp(10.0, track)` panics when track < 10.0
  - (track = text_h.max(1.0), so text_h in [1.0, 10.0) triggers it)
  - This happens on very short windows where the minimap text area shrinks below 10px

### 18:40 - Bug fix: minimap_viewport clamp (bug class: clamp with min > max)
- Location: crates/helion-gui/src/bin/helion-ide/editor.rs:686
- Fix: `clamp(10.0, track)` -> `clamp(10.0_f32.min(track), track)`
- Also fixed the same pattern in the test expectation (line 728)
- Added regression test `minimap_thumb_tiny_track_no_panic` covering text_h in [1.0, 9.9]
- Verified: crash-59c2055905bbbe0e now replays clean
- Swept for same class: only 2 instances of `clamp(10.0,` in editor.rs, both fixed
- Removed #[ignore] from all 3 crash regression tests; they now actively guard the fixes

### 18:45 - Main fuzz run started
- Seeds: 1000-1029 (30 seeds), 20000 frames each, deadline 7200s (2 hours)
- Log dir: /tmp/fuzz-main
- Command: HELION_FUZZ_RUN=1 HELION_FUZZ_SEED=1000 HELION_FUZZ_SEEDS="1001-1029" HELION_FUZZ_FRAMES=20000 HELION_FUZZ_DEADLINE_SECS=7200 cargo test -p helion-gui --bin helion-ide fuzz_run
