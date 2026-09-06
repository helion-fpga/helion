# FM-HEL-TOP — Air paint/synth idle budgets (#3)

**Date:** 2026-09-06 ~02:45 America/New_York (UTC-4)  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Shipped (cheap idle docs):** SHA `c5f02b6` (`c5f02b6fda7d4b02f193de8ec483d2fdebce4592`)  
**Gold:** `WNS_PS=9640` **held** (shared tree; timing path untouched by Air)

## Goal

Profile box Air (helion-ide / eframe) for `request_repaint` / continuous loops; document idle budgets. Mac when up is out of scope if unreachable. Only ship cheap chrome/perf if clearly cuts idle.

## Mac scope

| Check | Result |
|-------|--------|
| ListMachines | Mac `sakshams-MacBook-Pro-517.local` `connected: true` |
| Shell via machineId | **temporarily unreachable** (one attempt) |
| Mac Air profile | **out of scope this turn** |

## Box audit findings

| Area | Finding |
|------|---------|
| `request_repaint` / `repaint_after` / `Continuous` in `helion-gui` | **none** (source grep) |
| `HelionIde::update` | paints toolbar/rail/sidebar/workspace/popups; **no** continuous timer |
| Synth / implement | **on click** via `run_step` / `implement()` — not a background paint loop |
| Floorplan `paint_device` | O(pins + bank rects) per **input** frame only when Device canvas visible |
| eframe `NativeOptions` | defaults (reactive); glow + wayland/x11; no custom vsync spin |
| DISPLAY | `:11` present; no forced continuous FPS path |

**Conclusion:** Air is already **idle-clean** on the reactive egui path (aligns with prior `FM-HEL-TOP-tap-or-air` skip of Air bar). No uncapped animation; synth does not spin in `update`.

## Budgets (documented)

Added to `crates/helion-gui/src/chrome.rs`:

| Constant | Value | Meaning |
|----------|-------|---------|
| `IDLE_PAINT_POLICY` | `reactive-no-request_repaint` | Event-driven paint only |
| `IDLE_CPU_SOFT_PCT` | `1` | Soft target ~0–1% CPU when unfocused / no input |
| `IDLE_ANIM_HZ` | `0` | No animation timer |

Doc comment on `HelionIde::update`: reactive eframe; synth on click.  
Test: `chrome::idle_budget_tests::idle_policy_is_reactive_no_continuous_anim`.

Existing layout budgets (unchanged): `RAIL_WIDTH=48`, `SIDEBAR_WIDTH=240`, `TABLE_MAX_HEIGHT=180`, `DEVICE_TABLES_MAX_HEIGHT=220`, `DRAWING_MIN_HEIGHT=280`, desktop 1440×900.

## Chrome/perf ship

**Cheap only:** idle-policy constants + comments + one unit test. **No** decoration overkill; no structural paint rewrite (would not cut idle further).

## What remains

1. When Mac local-exec is up: Instruments / Activity Monitor idle CPU with Helion focused vs unfocused.
2. If a future feature adds live waveform scroll / progress animation, gate it with `request_repaint` **only while active**, then stop (keep `IDLE_ANIM_HZ=0` default).
3. Optional: measure floorplan paint cost under resize spam (not idle).

## Verdict

**PASS (budgets documented; Mac unreachable)** — Box Air has no continuous repaint; synth idle is click-driven; soft budgets recorded; cheap docs/tests only; gold 9640; no merge.
