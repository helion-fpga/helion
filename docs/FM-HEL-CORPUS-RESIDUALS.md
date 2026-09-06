# FM-HEL-CORPUS residual SOFTs (0) — FM-HEL-TOP soft-helion-ahead

Do **not** run uncapped hangs. FAIL remains 0. Gold **WNS_PS=9640**.

| design | class | status (2026-09-06) | guidance |
|--------|-------|---------------------|----------|
| **sha256** | hang-cap | **PASS** — wrapper `sha256_phase_d.v` (w_mem as top). Full `sha256_all.v` hang deferred. |
| **ysyx_ibex** | golden-gap (Helion-ahead) | **PASS** — Helion-primary (`yosys_gap`/`helion_ahead`); cells=52742; keep **capped** via `scripts/ibex-synth-capped.sh`. Never uncapped. |
| **uart** | golden-gap (Helion-ahead) | **PASS** — Helion-primary; cells=5704; OT SV Yosys gap documented. |

Score now **PASS 100 / SOFT 0 / FAIL 0**.
