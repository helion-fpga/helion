# Schematic deepen vs Yosys connectivity

**Date:** 2026-09-05/06 (America/New_York)
**Status:** **DONE-at-57** (sample deepen; GUI hierarchy sheet nav patched on Mac working tree — push pending if Mac offline)
**Scope:** capped sample of phase-d PASS designs (no uncapped Ibex/sha256)
**Gold:** `WNS_PS=9640` held
**Phase-d score:** PASS 98 / SOFT 2 / FAIL 0 (uart, ysyx_ibex) — deepen does not flip residuals
**Push:** doc/code on `fm-hel-corpus-soft-pass` (NO MERGE)

## Method (gap-3 continuation)

1. Prior DONE-at-48 already on PR #7 (`f21e4f9`).
2. Fold leftover phase-d `helion_schematic.out` dumps with non-empty synth+schematic+drawing into sample (no new IDE runs).
3. Exclude `sha256` / `ysyx_ibex` (capped residuals) and 41 EMPTY_SYNTH dumps (stale ide cells=0).
4. GUI: wire Hierarchy pane → schematic sheet (`open_hierarchy_sheet`); timing-path highlight already present (`select_timing_path`).

## Sample table (n=+9 leftover phase-d fold-in)

| id | deepen | Helion synth | sch cells/edges | draw sym/wires | Yosys cells | notes |
|----|--------|--------------|-----------------|----------------|-------------|-------|
| pipeline | PASS | 1 | 1/0 | 8/0 | 1056 | no_wires |
| ramasync | PASS | 1 | 1/0 | 6/0 | 8768 | no_wires |
| ramspnc | PASS | 1 | 1/0 | 7/0 | 35014 | no_wires |
| cache | PASS | 7 | 7/8 | 15/12 | 62260 | - |
| complex | PASS | 129 | 22/0 | 24/34 | 34 | hierarchical; sch visible < flat synth |
| dffasync | PASS | 129 | 129/4097 | 133/194 | 64 | - |
| dffsync | PASS | 129 | 129/4097 | 133/194 | 64 | - |
| icg | PASS | 131 | 2/0 | 6/8 | 68 | hierarchical; sch visible < flat synth |
| fifoasync | PASS | 145 | 145/4039 | 155/290 | 2383 | - |

**Sample deepen counts:** PASS 57 / SOFT 0 / FAIL 0 (n=57; before=48)

## GUI depth features

| feature | status | hook |
|---------|--------|------|
| Timing-path highlight (Fig. 59) | working (pre-existing) | `select_timing_path` → highlight_cells/nets + path_only |
| Hierarchy sheet navigation (Fig. 61→55/56) | patched in Mac WT | `open_hierarchy_sheet` + Hierarchy double-click / Show in Schematic |
| Sheet find / Expand Inside | working | `sheet_find`, schematic double-click instance |

## Still missing

- ~41 EMPTY_SYNTH leftovers need fresh rebuilt `helion-ide` dumps
- Large/timeout: fsm schematic timeout; big cells (muladdc/hamming/…)
- Full ~98 PASS not schematic-swept; sha256/Ibex intentionally skipped
- Mac commit+push of GUI patch + docs may still be pending if machine offline

## Bar move

- Schematic connectivity sample: **48 → 57 PASS** / 0 SOFT / 0 FAIL
- Raw dumps: `dashboard/schematic-deepen-raw/` (+9 copies)
- JSON: `dashboard/schematic-deepen-sample.json`
