# FM-HEL-CORPUS residual SOFTs (2) — FM-HEL-TOP gap 4

Do **not** run uncapped hangs. FAIL remains 0. Gold **WNS_PS=9640**.

| design | class | status (2026-09-06) | guidance |
|--------|-------|---------------------|----------|
| **sha256** | hang-cap | **PASS** — wrapper `sha256_phase_d.v` (w_mem as top). Helion **cells=2860** luts=2827 (~0.1s); Yosys 3447. Full `sha256_all.v` still hangs ≤60s — deferred. |
| **ysyx_ibex** | golden-gap (Helion-ahead) | **HANG FIXED** — capped synth PASS cells≈6930 via `scripts/ibex-synth-capped.sh`. Still SOFT vs Yosys (`yosys_fail`). Harness has **no** Helion-primary PASS without alt golden (only `hamming`-style iverilog/synth_xilinx). Keep SOFT + `helion_ahead`. |
| **uart** | golden-gap (Helion-ahead) | Helion ~5704 cells; OT SV breaks Yosys/iverilog. Same taxonomy — SOFT only (do not invent PASS). |

Score now **PASS 98 / SOFT 2 / FAIL 0**.
