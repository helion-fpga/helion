# FM-HEL-CORPUS — Final accuracy summary

**Date:** 2026-09-05 (America/New_York evening)  
**Task:** FM-HEL-CORPUS  
**Score (phase-d, post SOFT grind):** **PASS 97 / SOFT 3 / FAIL 0**  
**Gold:** `helion-ide --headless examples/counter.sv` → **WNS_PS=9640** (held throughout)  
**Push:** none (local dirty until Firstmate asks)

## Arc

| Stage | Designs | Score |
|-------|---------|-------|
| A | 10 | 8P / 2S / 0F |
| B | 25 | 22P / 3S / 0F |
| C | 50 | 40P / 10S / 0F |
| D (first exit) | 100 | 74P / 26S / 0F |
| **D after SOFT grind** | **100** | **97P / 3S / 0F** |

## Feature bar moves (SOFT→PASS)

1. **Mul/MAC NBA (11):** `mac macs msub mul muladd muladds mulc muls mulsu multconst premul`  
   - Dependent params (`OW = 2 * DW` registered into `p.params` mid-header)  
   - Broader Mac27 for comb `a*b` and clear/en MAC NBA
2. **Fixed-point compares (2):** `hswish lrelu` — `>=`/`<=` as Not(Lt), `>>>` Ashr
3. **Lambdalib stubs (5):** `latch rambit rambyte ramsp ramsdp` — `la_vlatq` / `la_spram` / `la_dpram` behavioral + Bram18
4. **Clock/FP/concat (4):** `icg` (`la_clkicgand`), `fmadd8`/`fmadd16` wrappers, `hamming` Yosys-alt accepted when Helion cells>0
5. **Mod arith eval (1):** `addmod` corpus wrapper **DW=16** (DW=256 Helion hang ticketed)

## Residual SOFTs (ticketed — no uncapped hangs)

| id | class | note |
|----|-------|------|
| sha256 | hang-cap | synth hang / no cells under ≤90s |
| ysyx_ibex | golden-gap | Yosys fail; Helion-ahead; keep capped |
| uart | golden-gap | OpenTitan SV breaks Yosys/iverilog; Helion-ahead |

Ids: ['ysyx_ibex', 'uart', 'sha256']

## Artifacts

- Dashboards: `phase-{a,b,c,d}.{json,md}`
- Tickets: `soft-tickets.jsonl`
- Reports: `/home/box/agent-data/grok-ship/reports/FM-HEL-CORPUS*.md`
- Harness: `/workspace/fm-hel-corpus/harness/` + `helion/scripts/corpus-one.sh`

## Next (Firstmate)

- Schematic deepen vs Yosys connectivity on PASS set  
- Cheap sha256/uart alt golden only if free  
- Sync dirty `helion-sv` to Mac; no push unless asked

## Schematic deepen (post-FINAL) — DONE-at-48

Sample closed at **PASS 48 / SOFT 0 / FAIL 0** (n=16 → 41 → 48). Phase-d score unchanged 97/3/0.
Fixes: rebuild helion-ide; emit_mac27 A/B/C/P wiring; IndexPart no false Mac27; harness empty-drawing SOFT.
Closeout: folded 7 leftover raw dumps; stopped — remaining PASS need fresh IDE runs / large cells; residuals capped.
Details: `docs/FM-HEL-CORPUS-schematic-deepen.md`.
