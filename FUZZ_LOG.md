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
