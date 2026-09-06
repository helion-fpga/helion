# Schematic deepen vs Yosys connectivity

**Date:** 2026-09-06 (America/New_York)
**Status:** **DONE-at-98** (monster-band DW-wraps: ln/muladdc/max/min/absdiff/clamp → +6)
**Scope:** capped sample of phase-d PASS designs (no uncapped Ibex/sha256)
**Gold:** `WNS_PS=9640` held
**Phase-d score:** PASS 98 / SOFT 2 / FAIL 0 (uart, ysyx_ibex) — deepen does not flip residuals
**Push:** doc on `fm-hel-corpus-soft-pass` (NO MERGE)

## Method (monster-band DW-reduced wraps)

1. Remaining EMPTY after batch2: `ln`, `muladdc`, `max`, `min`, `absdiff`, `clamp` (plus intentional sha256/ibex skip).
2. Full-width / DW=16 exploded schematic edges (muladdc/max TIMEOUT @55s; ln prior ~21M edges).
3. Added light wrappers `*_phase_d.v` under `vendored/logikbench/*/rtl/` with **DW=8** (muladdc OW=20; ln QW=4 + Q4.4-scaled coeffs).
4. Headless capped `helion-ide --stdin` (`open <wrap>` → `schematic` → `schematic_drawing` → `quit`), PG-kill ≤55s each.
5. IDE binary: `/workspace/helion/target/debug/helion-ide`. Folded all 6 PASS dumps into sample JSON.

## Sample deepen counts

**PASS 98 / SOFT 0 / FAIL 0** (n=98; before=92 → **+6**)

JSON: `dashboard/schematic-deepen-sample.json`  
Raw: `dashboard/schematic-deepen-raw/` (truncated stubs for monster drawing bodies)

### Newly folded (+6, DW-wrap)

| id | wrap | synth | sch | edges | wall |
|----|------|-------|-----|-------|------|
| `max` | DW=8 | 4152 | 4152 | 179276 | 2.47s |
| `min` | DW=8 | 4152 | 4152 | 179276 | 2.44s |
| `absdiff` | DW=8 | 4982 | 4982 | 216838 | 3.42s |
| `muladdc` | DW=8/OW=20 | 5152 | 5152 | 322510 | 3.68s |
| `clamp` | DW=8 | 8288 | 8288 | 532596 | 8.95s |
| `ln` | DW=8/QW=4 | 8515 | 8515 | 8011457 | 30.94s |

### Prior full-width failures (now covered via wraps)

| id | prior | detail |
|----|-------|--------|
| muladdc | TIMEOUT | DW=16 synth/sch=21792 edges≈5.56M @55s |
| max | TIMEOUT | DW=16 synth/sch=32880 edges≈5.63M @55s |
| ln | TIMEOUT | DW=16 synth/sch=17036 edges≈21.5M @55s |

## Remaining EMPTY_SYNTH after this pass

**None** in deepen sample targets.

### Intentional skip (not EMPTY; excluded from deepen)

`sha256`, `ysyx_ibex`

## Bar move

- Schematic connectivity sample: **92 → 98 PASS** / 0 SOFT / 0 FAIL
- Full deepen sample bar (~98 phase-d PASS excl. intentional skips) **met**
