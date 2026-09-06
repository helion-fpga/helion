# Schematic deepen vs Yosys connectivity

**Date:** 2026-09-06 (America/New_York)
**Status:** **DONE-at-92** (batch2 defer-band EMPTY_SYNTH regen: +18; muladdc/max TIMEOUT)
**Scope:** capped sample of phase-d PASS designs (no uncapped Ibex/sha256)
**Gold:** `WNS_PS=9640` held
**Phase-d score:** PASS 98 / SOFT 2 / FAIL 0 (uart, ysyx_ibex) — deepen does not flip residuals
**Push:** doc on `fm-hel-corpus-soft-pass` (NO MERGE)

## Method (batch2 defer-band EMPTY_SYNTH regen)

1. Diffed deepen sample PASS (n=74) vs remaining EMPTY_SYNTH; ranked defer-23 by phase-d helion cells.
2. Picked next ≤20 cheapest; skipped ln / sha256 / ysyx_ibex / min / absdiff / clamp (monsters / prior bomb).
3. Headless capped `helion-ide --stdin` on box (`open <rtl>` → `schematic` → `schematic_drawing` → `quit`), PG-kill ≤55s each.
4. IDE binary: `/workspace/helion/target/debug/helion-ide`. RTL from `fm-hel-corpus/vendored/logikbench/...`.
5. Folded non-empty dumps into `dashboard/schematic-deepen-sample.json`; raw under `dashboard/schematic-deepen-raw/`.

## Sample deepen counts

**PASS 92 / SOFT 0 / FAIL 0** (n=92; before=74 → **+18**)

JSON: `dashboard/schematic-deepen-sample.json`  
Raw: `dashboard/schematic-deepen-raw/` (92+ residual/timeout stubs)

### Newly folded (+18)

`divs`, `rom`, `raminit`, `rambit`, `rambyte`, `ramsdp`, `ramsp`, `crc32`, `cmp`, `addsub`, `addmod`, `uart`, `hswish`, `hamming`, `maxn`, `muxcase`, `add`, `macc`

### Batch2 failures

| id | result | detail |
|----|--------|--------|
| muladdc | TIMEOUT | synth/sch=21792/21792 edges≈5561040; drawing killed @55.57s (out≈207978627). Not folded. |
| max | TIMEOUT | synth/sch=32880/32880 edges≈5628120; drawing killed @55.53s (out≈198242614). Not folded. |

## Remaining EMPTY_SYNTH after this pass

### Still need regen / hang risk

`ln` (prior TIMEOUT ~17k cells / ~21M edges), `muladdc` (TIMEOUT edges≈5.56M), `max` (TIMEOUT edges≈5.63M)

### Deferred monsters (not in this ≤20)

`min`, `absdiff`, `clamp`  
Plus intentional skip: `sha256`, `ysyx_ibex` (not EMPTY; excluded from deepen sample).

## Bar move

- Schematic connectivity sample: **74 → 92 PASS** / 0 SOFT / 0 FAIL
- Full ~98 still needs monster band + ln-class
