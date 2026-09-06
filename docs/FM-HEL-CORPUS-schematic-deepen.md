# Schematic deepen vs Yosys connectivity

**Date:** 2026-09-06 (America/New_York)
**Status:** **DONE-at-57** (expand residual: no new fold-ins; IDE regen deferred)
**Scope:** capped sample of phase-d PASS designs (no uncapped Ibex/sha256)
**Gold:** `WNS_PS=9640` held
**Phase-d score:** PASS 98 / SOFT 2 / FAIL 0 (uart, ysyx_ibex) — deepen does not flip residuals
**Push:** doc on `fm-hel-corpus-soft-pass` (NO MERGE)

## Method (gap-3 expand attempt)

1. Prior DONE-at-57 already on branch (`docs/FM-HEL-CORPUS-schematic-deepen.md`).
2. Re-scanned all phase-d `helion_schematic.out` not in sample:
   - foldable non-empty leftovers: **only** `sha256`, `ysyx_ibex` → **excluded** (capped / intentional skip)
   - **41 EMPTY_SYNTH** stale dumps (`synth cells=0`) — need fresh `helion-ide --stdin`
3. Box has `helion-ide` at `/workspace/helion/target/debug/helion-ide`; Mac unreachable this pass.
4. Expand steered to **ship now** (no multi-design IDE batch this turn).

## Sample deepen counts

**PASS 57 / SOFT 0 / FAIL 0** (n=57; before=57 — no move)

JSON: `dashboard/schematic-deepen-sample.json`  
Raw: `dashboard/schematic-deepen-raw/` (58 files; `fsm.out` is TIMEOUT cells=39628 — not in sample)

## Remaining EMPTY_SYNTH (41) — skip / regen map

### Cheap regen candidates (helion cells ≤500) — 18

`addtree`(149), `bin2prio`(131), `bnor`(128), `bor`(129), `bxnor`(442), `bxor`(443), `cam`(85), `csa42`(97), `dec`(312), `div`(134), `fsm`(17), `ln`(87), `log2`(170), `mod`(118), `muxhot`(129), `muxpri`(129), `serv`(321), `tmr`(128)

### Skip / defer (>500 or SOFT / hang risk) — 23

`absdiff`, `add`, `addmod`, `addsub`, `clamp`, `cmp`, `crc32`, `divs`, `hamming`, `hswish`, `macc`, `max`, `maxn`, `min`, `muladdc`, `muxcase`, `rambit`, `rambyte`, `raminit`, `ramsdp`, `ramsp`, `rom`, `uart`  
Plus intentional skip: `sha256`, `ysyx_ibex` (not EMPTY; excluded from deepen sample).

## Blockers

- Fresh schematic dumps require headless IDE runs; expand turn capped to ship without batch regen.
- Mac local-tool unreachable for GUI WT push this pass.
- Full ~98 PASS not schematic-swept until cheap-18 regenerated + folded.

## Bar move

- Schematic connectivity sample: **57 → 57 PASS** / 0 SOFT / 0 FAIL (held)
- Next lever: regenerate cheap-18 EMPTY_SYNTH with capped/headless `helion-ide`, fold into sample → target n≈75
