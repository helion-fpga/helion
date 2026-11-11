# Schematic deepen vs Yosys connectivity

**Date:** 2026-09-06 (America/New_York)
**Status:** **DONE-at-99** (sha256 phase_d wrap folded → +1; residual intentional skip: ysyx_ibex only)
**Scope:** capped sample of phase-d PASS designs (no uncapped Ibex)
**Gold:** `WNS_PS=9640` held
**Phase-d score:** PASS 100 / SOFT 0 / FAIL 0 (uart, ysyx_ibex, sha256 SOFTs closed) — deepen does not flip residuals
**Push:** doc on `fm-hel-corpus-soft-pass` (NO MERGE)

## Method (this pass)

1. Prior bar: DONE-at-98 (monster-band DW-wraps: ln/muladdc/max/min/absdiff/clamp).
2. Leftover intentional skips: `sha256`, `ysyx_ibex`.
3. Cheap fold: headless capped `helion-ide --stdin` on `sha256_phase_d.v` (w_mem top), PG-kill ≤55s — completed in **~2.4s**.
4. `ysyx_ibex` still excluded (never uncapped cargo / schematic hang).

## Sample deepen counts

**PASS 99 / SOFT 0 / FAIL 0** (n=99; before=98 → **+1**)

JSON: `dashboard/schematic-deepen-sample.json`  
Raw: `dashboard/schematic-deepen-raw/` (truncated stubs for monster drawing bodies)

### Newly folded (+1)

| id | wrap | synth | sch | edges | sym | wires | wall |
|----|------|-------|-----|-------|-----|-------|------|
| `sha256` | `sha256_phase_d.v` (w_mem) | 2860 | 2860 | 484954 | 2867 | 2827 | 2.40s |

### Prior monster-band DW-wraps (+6, still held)

| id | wrap | synth | sch | edges | wall |
|----|------|-------|-----|-------|------|
| `max` | DW=8 | 4152 | 4152 | 179276 | 2.47s |
| `min` | DW=8 | 4152 | 4152 | 179276 | 2.44s |
| `absdiff` | DW=8 | 4982 | 4982 | 216838 | 3.42s |
| `muladdc` | DW=8/OW=20 | 5152 | 5152 | 322510 | 3.68s |
| `clamp` | DW=8 | 8288 | 8288 | 532596 | 8.95s |
| `ln` | DW=8/QW=4 | 8515 | 8515 | 8011457 | 30.94s |

## Remaining residual (honest)

### Intentional skip (not EMPTY; excluded from deepen)

| id | reason |
|----|--------|
| `ysyx_ibex` | Never uncapped cargo / schematic; Helion-primary PASS via capped synth (cells=52742). Deepen sample residual **1**. |

EMPTY_SYNTH leftovers in deepen sample targets: **none**.

## Soft verify (same session)

| check | result |
|-------|--------|
| phase-d score | **PASS 100 / SOFT 0 / FAIL 0** |
| uart / ysyx_ibex / sha256 | all **overall PASS** (cells 5704 / 52742 / 2860) |
| harness Helion-primary | present (`yosys_gap` / `helion_ahead`) |
| gold | `helion report_timing examples/counter.sv` → **WNS_PS=9640** |

## Bar move

- Schematic connectivity sample: **98 → 99 PASS** / 0 SOFT / 0 FAIL
- Honest residual deepen skip: **1** (`ysyx_ibex`)
