# Schematic deepen vs Yosys connectivity

**Date:** 2026-09-06 (America/New_York)
**Status:** **DONE-at-74** (cheap-18 EMPTY_SYNTH regen: +17; ln TIMEOUT)
**Scope:** capped sample of phase-d PASS designs (no uncapped Ibex/sha256)
**Gold:** `WNS_PS=9640` held
**Phase-d score:** PASS 98 / SOFT 2 / FAIL 0 (uart, ysyx_ibex) — deepen does not flip residuals
**Push:** doc on `fm-hel-corpus-soft-pass` (NO MERGE)

## Method (cheap-18 EMPTY_SYNTH regen)

1. Listed cheap-18 ≤500-cell EMPTY_SYNTH from deepen residual map.
2. Headless capped `helion-ide --stdin` on box (`open <rtl>` → `schematic` → `schematic_drawing` → `quit`), PG-kill ≤55s each.
3. IDE binary: `/workspace/helion/target/debug/helion-ide`. RTL from `fm-hel-corpus/vendored/logikbench/...`.
4. Folded non-empty dumps into `dashboard/schematic-deepen-sample.json`; copied raw outs under `dashboard/schematic-deepen-raw/`.
5. Note: current IDE cell counts drift vs older phase-d summaries for some IDs (cam/fsm/muxhot/serv/ln); still folded when dump completed with synth>0 & sch>0.

## Sample deepen counts

**PASS 74 / SOFT 0 / FAIL 0** (n=74; before=57 → **+17**)

JSON: `dashboard/schematic-deepen-sample.json`  
Raw: `dashboard/schematic-deepen-raw/` (74+ residual files; `ln` TIMEOUT not folded)

### Newly folded (+17)

`addtree`, `bin2prio`, `bnor`, `bor`, `bxnor`, `bxor`, `cam`, `csa42`, `dec`, `div`, `fsm`, `log2`, `mod`, `muxhot`, `muxpri`, `serv`, `tmr`

### Cheap-18 failures

| id | result | detail |
|----|--------|--------|
| ln | TIMEOUT | synth/sch cells=17036 edges≈21M; drawing killed @55s (out≈760MB). Not folded. |

## Remaining EMPTY_SYNTH after this pass

### Still need regen

`ln` (cheap-band on older helion; IDE now ~17k cells — drawing hang)

### Skip / defer (>500 or SOFT / hang risk) — 23

`absdiff`, `add`, `addmod`, `addsub`, `clamp`, `cmp`, `crc32`, `divs`, `hamming`, `hswish`, `macc`, `max`, `maxn`, `min`, `muladdc`, `muxcase`, `rambit`, `rambyte`, `raminit`, `ramsdp`, `ramsp`, `rom`, `uart`  
Plus intentional skip: `sha256`, `ysyx_ibex` (not EMPTY; excluded from deepen sample).

## Bar move

- Schematic connectivity sample: **57 → 74 PASS** / 0 SOFT / 0 FAIL
- Target n≈75 essentially met (74); full ~98 still needs defer-band + ln
