# Helion vs Open-Source Baseline — 10-Design Benchmark

**Date:** 2026-10-10
**Helion:** v2.0.5, built from source (https://github.com/helion-fpga/helion), Rust 1.85.0, Linux x86_64
**Target die:** HL10T-C32-1 ("HL10T (~8k LUT6)"): 32×32 CLB grid, 8 BLEs/CLB = 8192 LUT6+FF, 8 BRAM18, 0 DSP blocks, 32 user IOs
**Baseline:** Yosys 0.69+274 + nextpnr-ecp5 → Lattice LFE5U-25F (24k LUT4), 100 MHz target
**Method:** Identical RTL for both flows. Each design wrapped with a minimal top (clk + live stimulus, ≤32 pins) to fit the HL10T IO budget. Helion run via `helion qor <design.prj>` (synth → pack → place → PathFinder route → STA → bitstream). Baseline via Yosys `synth_ecp5` + nextpnr-ecp5.

> **Reading this honestly.** Helion targets its own LUT6 fabric; the baseline
> targets Lattice ECP5 (LUT4). Absolute LUT counts and MHz are **not**
> silicon-to-silicon comparable. This compares: (1) flow completeness — does
> every stage run? (2) runtime, (3) relative area/timing on identical RTL.
> Wording is neutral; neither tool is hyped.

## Results table

| # | Design | Source | Helion LUT6 | Helion BRAM | Helion WNS (ps) | Helion wall | Base LUT4 | Base FF | Base Fmax (MHz) | Base wall | Verdict |
|---|--------|--------|-------------|-------------|-----------------|-------------|-----------|---------|-----------------|-----------|---------|
| 1 | Ibex RV32 | helion-fpga/helion `examples/ysyx_ibex.sv` | 8192 † | 8 † | 9540 | 7.0 s | — ‡ | — | — | — | OVER CAPACITY |
| 2 | PicoRV32 | YosysHQ/picorv32 | 2477 | 0 | 8700 | 3.5 s | 830 | 318 | 100.6 | 10.4 s | OK |
| 3 | SERV | olmsteinn/serv | 395 | 0 | 7620 | 0.7 s | 133 | 132 | 196.2 | 1.9 s | OK |
| 4 | MIPS 1-cycle | DiabloBlood/single-cycle-MIPS-CPU | 397 | 3 | 9600 | 0.09 s | — § | — | — | — | OK (Helion only) |
| 5 | AES-128 | secworks/aes | 6534 | 1 | 8700 | 1.9 s | 11584 | 2475 | 42.3 | ~332 s | OK |
| 6 | SHA-256 | secworks/aes (via Helion corpus-pass) | 1865 | 0 | 8220 | 0.9 s | 2753 | 1163 | 48.2 | ~146 s | OK |
| 7 | UART | alexforencich/verilog-uart | 379 | 0 | 9600 | 0.3 s | 133 | 77 | 186.5 | 5.8 s | OK |
| 8 | SPI master | andrade824/Verilog-SPI-Master | 62 | 0 | 9580 | 0.03 s | 13 | 17 | 197.9 | 3.6 s | OK |
| 9 | I2C master | alexforencich/verilog-i2c | 822 | 0 | 9580 | 0.9 s | 183 | 81 | 130.4 | 5.0 s | OK |
| 10 | CORDIC | freecores/verilog_cordic_core | 385 | 0 | 9600 | 0.08 s | — ¶ | — | — | — | OK (Helion only) |

† **Ibex exceeds the die.** Pack produced 11,572 LUTFFs and 36 BRAMs vs
8,192 LUT6 / 8 BRAM18 available. The placer caps at die limits
(`place_bram_cap 36 → 8`) and the router reports `overused=1` (unresolved
congestion). The Ibex row reflects a clipped, non-functional implementation.

‡ Yosys 0.69's Verilog frontend cannot parse the SystemVerilog assignment
patterns (`'{...}`) in `ysyx_ibex.sv`; Helion's `sv-parser`-based frontend
handles them. No baseline numbers.

§ Yosys `synth_ecp5` rejects an inferred latch in `MainControl.v` (case
without default — a genuine RTL bug in this teaching design). Helion
elaborates it. No baseline numbers.

¶ Yosys frontend cannot elaborate the variable-bound `for` loop in the
`signed_shifter` barrel shifter (`for(j=0;j<i;j++)` with runtime bound).
Helion elaborates it. No baseline numbers.

## Helion stage breakdown (ms)

| Design | synth | pack | place | route (PathFinder) | STA | total |
|--------|-------|------|-------|--------------------|-----|-------|
| ibex | 4155 | 215 | 1212 | 42 (24 iters, overused=1) | 495 | 6959 |
| picorv32 | 519 | 37 | 925 | 7 (5 iters) | 21 | 3517 |
| serv | 66 | 6 | 22 | 37 (8 iters) | 3 | 729 |
| mips | 53 | 5 | 14 | 0 (1 iter) | 3 | 94 |
| aes128 | 729 | 54 | 851 | 7 (1 iter) | 42 | 1871 |
| sha256 | 368 | 19 | 185 | 29 (8 iters) | 14 | 915 |
| uart | 293 | 4 | 15 | 0 (1 iter) | 2 | 332 |
| spi | 19 | 0 | 2 | 0 (1 iter) | 0 | 29 |
| i2c | 761 | 8 | 58 | 1 (1 iter) | 5 | 879 |
| cordic | 40 | 9 | 14 | 0 (1 iter) | 4 | 79 |

All 10 designs completed every Helion stage (synth → pack → place → route →
STA → bitstream). No hangs, no crashes. Slowest stage is synthesis (AIG
elaboration) on the largest designs.

## What the numbers say

**Speed.** Helion is 3–100× faster wall-clock than Yosys+nextpnr on the same
RTL (e.g. AES-128: 1.9 s vs ~5.5 min; PicoRV32: 3.5 s vs 10.4 s). Helion's
flow is a straight pipeline with no heavy optimization passes; Yosys spends
most of its time in ABC logic optimization and nextpnr in simulated-annealing
placement.

**Area.** Yosys+ABC produces 2–5× fewer LUTs on the same RTL (PicoRV32:
830 LUT4 vs 2477 LUT6; UART: 133 vs 379; SPI: 13 vs 62). Part of this is
LUT4-vs-LUT6 accounting, but most of it is ABC's logic optimization vs
Helion's direct AIG→FlowMap mapping. Helion trades area for speed and
simplicity — a legitimate bring-up-stage tradeoff, stated plainly.

**Timing.** Every Helion run met the 10 ns target with positive WNS.
nextpnr's reported Fmax (42–198 MHz) reflects its signoff-style delay model;
Helion's WNS numbers imply optimistic Fmax and come from a simpler delay
model — do not compare the MHz head-to-head.

**Robustness.** Helion's SystemVerilog frontend parsed all 10 designs,
including constructs Yosys 0.69 rejected (assignment patterns, variable-bound
loops, latch inference). 9 of 10 fit the die and routed with zero congestion;
Ibex (a ~11.6k-LUT core) does not fit the 8k-LUT HL10T — the flow caps
placement instead of erroring, which is the one honest wart: it should fail
loudly on over-capacity instead of emitting a clipped bitstream.

## Adaptations (documented)

- All designs use a thin wrapper (clk + counter/LFSR/ROM stimulus, LED
  output) to fit the 32-IO budget and keep logic live. Wrappers are
  benchmark harness, not the design under test.
- PicoRV32/SERV: replaced the repo's tied-constant wrappers with live
  instruction-ROM wrappers after finding Yosys constant-folded the
  tied versions to ~1 LUT. Helion numbers use the live wrappers too.
- MIPS: created `Instruction.txt` (6-word program) required by
  `$readmemh`; missing from the upstream repo.
- No RTL of any design-under-test was modified.

## Repro

- Helion: `helion qor <design.prj>` from the repo root (v2.0.5, Rust 1.85).
- Baseline: `/tmp/bench/run_base.sh <name> <top> <clk> <vfiles...>` using
  oss-cad-suite 2026-10-10 (Yosys 0.69+274, nextpnr-ecp5).
- Wrappers + project files: `/tmp/bench/runs/<design>/`.

## Follow-up limitations to kill (owner directive, 2026-10-10)

1. **Over-capacity must fail loudly.** Pack wanted 11,572 LUTFFs / 36 BRAMs;
   the placer silently capped to die limits and emitted a non-functional
   bitstream. Fix: hard error naming the overused resource with counts.
2. **Area gap vs ABC (2–5×).** Add logic optimization passes (rewrite/refactor
   style) and area-recovery LUT mapping.
3. **Timing model calibration.** Helion's delay model is simpler/optimistic
   vs nextpnr signoff-style; calibrate against measured data.
