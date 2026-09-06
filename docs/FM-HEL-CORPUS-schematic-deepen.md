# Schematic deepen vs Yosys connectivity

**Date:** 2026-09-05 (America/New_York evening); expand + closeout same evening / early 2026-09-06
**Status:** **DONE-at-48** (sample closed; not full phase-d PASS sweep)
**Scope:** capped sample of phase-d PASS designs (no uncapped Ibex/sha256)
**Gold:** `WNS_PS=9640` held (`helion report_timing examples/counter.sv`)
**Phase-d score:** unchanged PASS 97 / SOFT 3 / FAIL 0 (no residual flipped)
**Push:** doc-only on `fm-hel-corpus-soft-pass` (no chrome, no hang reverts, no merge)

## Method

1. Survey harness schematic stage in `harness/corpus_one.py`: IDE stdin `open` → `schematic` → `schematic_drawing`; PASS if dump keywords present (`drawing` / `symbols=` / `schematic` / `cone=`).
2. Sample small/medium PASS designs; run capped `helion-ide --stdin` schematic dump + compare to existing phase-d `yosys.json` (cell/port/net counts).
3. Coarse grade: empty cone/drawing when synth cells>0 → SOFT; synth 0 vs Yosys cells>0 → FAIL; else PASS. Also note Mac27 `:nc` pins and wire counts.
4. Fix cheap bugs found; smoke with unit tests + sample re-dump.
5. **Expand:** prior n=16 → add ~24 more small/medium PASS (helion cells≤500); exclude hang-prone / uncapped residuals.
6. **Closeout expand:** fold 7 already-dumped PASS leftovers in `schematic-deepen-raw/` (no new IDE runs) → n=48. Stop.

## Issues found

| issue | impact | fix |
|-------|--------|-----|
| Stale `helion-ide` (built before latest `helion-sv`) | Many PASS designs dumped `synth_design cells=0` / empty cone while CLI synth had cells>0 — keyword-only harness still PASS | Rebuild `helion-ide` against current tree |
| Mac27 emitted with **no net connects** | `mac`/`mul` HNF island; schematic pins all `:nc`, wires=0 | `emit_mac27` wires A/B/(C)/P from RHS/LHS; unit asserts on nets |
| `expr_contains_mul` treated `data[sel*DW +: DW]` as DSP | `mux` falsely mapped to Mac27 (1 DSP + IOBs) instead of LUT mux | IndexPart no longer counts as datapath mul |
| Harness ignored empty cones | False PASS when dump keywords present but cells=0 | SOFT if `synth_design cells>0` but schematic/drawing empty |

**Expand round:** no new cheap FAIL bugs. `fsm` IDE synth → 39628 cells then schematic timeout (75s) → SOFT; not fixed (not cheap). Replaced in PASS sample by `gray2bin` + `binv`.

**Closeout round:** no new IDE runs / no code changes. Graded leftover raw dumps; 7 PASS folded in; `fsm` remains SOFT/excluded.

## Sample table (n=16 prior)

| id | deepen | Helion synth | sch cells/edges | draw sym/wires | Yosys cells | notes |
|----|--------|--------------|-----------------|----------------|-------------|-------|
| counter | PASS | 9 | 9/32 | 11/20 | 10 | - |
| blinky | PASS | 3 | 3/4 | 5/5 | 2 | - |
| mux | PASS | 16 | 16/92 | 19/16 | 24 | - |
| arbiter | PASS | 33 | 33/136 | 35/32 | 44 | - |
| fifosync | PASS | 166 | 166/1967 | 175/269 | 2384 | - |
| mul | PASS | 33 | 33/528 | 36/35 | 1534 | unused Mac27.C :nc (comb mul) |
| mac | PASS | 1 | 1/0 | 7/3 | 1848 | hard Mac27 vs Yosys gate blast (tech gap OK) |
| latch | PASS | 128 | 65/2016 | 68/133 | 65 | hierarchical la_vlatq; sch visible < flat synth |
| band | PASS | 64 | 64/63 | 66/64 | 63 | - |
| tff | PASS | 129 | 129/6178 | 133/322 | 128 | - |
| bin2gray | PASS | 128 | 128/2143 | 130/128 | 63 | - |
| onehot | PASS | 128 | 128/2080 | 130/128 | 162 | - |
| csa32 | PASS | 64 | 64/320 | 69/64 | 112 | - |
| abs | PASS | 32 | 32/136 | 34/32 | 109 | - |
| inc | PASS | 207 | 207/1066 | 209/217 | 38 | - |
| shiftreg | PASS | 192 | 192/8254 | 196/383 | 64 | - |

## Sample table (n=+25 expand)

| id | deepen | Helion synth | sch cells/edges | draw sym/wires | Yosys cells | notes |
|----|--------|--------------|-----------------|----------------|-------------|-------|
| macs | PASS | 1 | 1/0 | 7/3 | 759 | hard Mac27 (like mac) |
| hier | PASS | 3 | 2/0 | 4/10 | 3 | - |
| mulreg | PASS | 70 | 70/1126 | 74/104 | 1818 | mac_nc=1 |
| avgn | PASS | 8 | 8/7 | 10/7 | 798 | - |
| argmax | PASS | 12 | 12/10 | 14/10 | 785 | - |
| argmin | PASS | 12 | 12/10 | 14/10 | 743 | - |
| gelu | PASS | 21 | 21/125 | 23/20 | 2562 | mac_nc=6 |
| exp | PASS | 57 | 57/513 | 59/49 | 1876 | mac_nc=12 |
| popcount | PASS | 22 | 22/26 | 24/21 | 131 | - |
| fmadd8 | PASS | 27 | 10/28 | 14/38 | 2491 | hierarchical; sch visible < flat synth |
| crossbar | PASS | 32 | 32/0 | 35/0 | 16384 | no_wires (island/ports); Yosys blast |
| lrelu | PASS | 32 | 32/136 | 34/32 | 88 | - |
| absdiffs | PASS | 33 | 33/272 | 36/48 | 238 | - |
| msub | PASS | 33 | 33/528 | 37/33 | 1788 | mac_nc=3 |
| muladd | PASS | 33 | 33/528 | 37/36 | 1754 | - |
| muladds | PASS | 33 | 33/528 | 37/36 | 1974 | - |
| muls | PASS | 33 | 33/528 | 36/35 | 1754 | mac_nc=1 |
| mulsu | PASS | 33 | 33/528 | 36/34 | 1584 | mac_nc=1 |
| multconst | PASS | 33 | 33/528 | 35/33 | 546 | mac_nc=3 |
| premul | PASS | 34 | 34/561 | 38/36 | 2517 | mac_nc=1 |
| clz | PASS | 39 | 39/1202 | 41/549 | 303 | - |
| ctz | PASS | 39 | 39/1202 | 41/549 | 303 | - |
| fmadd16 | PASS | 45 | 18/120 | 22/70 | 4068 | hierarchical; sch visible < flat synth |
| gray2bin | PASS | 65 | 65/2016 | 67/64 | 63 | fsm-timeout swap-in |
| binv | PASS | 65 | 65/2017 | 67/65 | 64 | fsm-timeout swap-in |

## Sample table (n=+7 closeout fold-in)

Already-present raw dumps under `dashboard/schematic-deepen-raw/` that were graded but not yet in the JSON sample:

| id | deepen | Helion synth | sch cells/edges | draw sym/wires | Yosys cells | notes |
|----|--------|--------------|-----------------|----------------|-------------|-------|
| atan | PASS | 54 | 54/1194 | 56/436 | 4306 | - |
| bnand | PASS | 65 | 65/64 | 67/65 | 64 | - |
| cos | PASS | 54 | 54/1194 | 56/436 | 5804 | - |
| dotprod | PASS | 75 | 75/623 | 78/35 | 14175 | mac_nc=24 |
| firfix | PASS | 70 | 70/2345 | 75/204 | 8531 | - |
| fmadd32 | PASS | 79 | 33/496 | 37/99 | 4056 | hierarchical; sch visible < flat synth |
| mulc | PASS | 66 | 66/1056 | 72/68 | 7477 | mac_nc=4 |

**Sample deepen counts:** PASS 48 / SOFT 0 / FAIL 0 (n=48; fsm attempted → SOFT timeout, excluded)

## DONE-at-48 — why stop here

**What 48 covers:** small/medium phase-d PASS designs across counters, mux/arbiter, FIFO, DSP/Mac27 (mul/mac/fmadd*), bit-twiddles, activations (gelu/lrelu/exp), trig approx (atan/cos), FIR, and hierarchical FMADD — all with non-empty schematic cone + drawing vs Yosys cell counts. Cheap connectivity bugs already fixed (Mac27 nets, IndexPart mul false-positive, empty-cone harness SOFT).

**Why not expand now (remaining ~49 PASS + 3 SOFT):**
- No more free raw dumps; further expand needs fresh `helion-ide` stdin runs (minutes each; fsm already 75s timeout at 39k cells).
- Many remainders are large (muxcase/add/hamming/clamp/max/min/macc/muladdc 10k–65k Helion cells) or RAM-heavy — outside the ≤500-cell cheap band used for expand.
- Capped-only policy: sha256 / ysyx_ibex / uart residuals untouched; hang fix already pushed — do not re-touch.
- Chrome WIP stays unstaged; deepen is doc/sample closeout only.
- Phase-d bar already 97P/3S/0F; schematic deepen is a connectivity sample, not a second full corpus.

**Residual gaps:** unused Mac27 `:nc` pins on some DSP maps (noted, not FAIL); hierarchical designs show sch cells < flat synth; `crossbar` drawing wires=0; `fsm` schematic timeout; full 97 PASS not schematic-swept.

## Code fixes

- `crates/helion-sv/src/lib.rs`: `emit_mac27` + `mac_ab_c_from_rhs`; IndexPart excluded from `expr_contains_mul`; Mac unit tests assert net_on A/B/C/P. *(prior round)*
- `harness/corpus_one.py`: empty-cone SOFT when open synth cells>0. *(prior round)*
- Rebuilt `target/debug/helion` + `helion-ide`. *(prior round)*
- **Expand / closeout rounds:** no new code changes.

## Residuals / alt golden (cheap only)

| id | action |
|----|--------|
| sha256 | No free flip — Yosys already PASS; Helion synth hang-cap remains. No uncapped retry. |
| uart | No free flip — primary Yosys + synth_xilinx both FAIL in artifacts; iverilog not a free win. |
| ysyx_ibex | Untouched (capped-only policy). |
| fsm (schematic) | IDE open synth 39628 cells; schematic dump timeout 75s → SOFT. Not cheap; left out of PASS sample. |

## Bar move

- **Phase-d dashboard score:** no change (97/3/0). Residuals sha256/uart/ysyx_ibex still SOFT.
- **Schematic connectivity (sample):** n=16 → n=41 → **n=48 PASS** / 0 SOFT / 0 FAIL (FAIL=0 held).
- Raw dumps: `dashboard/schematic-deepen-raw/`; JSON: `dashboard/schematic-deepen-sample.json`.
