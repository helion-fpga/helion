# FM-HEL-CORPUS residual SOFTs (3)

Do **not** run uncapped hangs. FAIL remains 0.

| design | class | ticket id | guidance |
|--------|-------|-----------|----------|
| **sha256** | hang-cap | `SOFT-sha256-synth_hang-residual` | Helion synth hangs / no cells under ≤90s. No uncapped retry. Cheap alt golden only if free. |
| **ysyx_ibex** | golden-gap (Helion-ahead) | `SOFT-ysyx_ibex-yosys_gap-residual` | Yosys fails; Helion large cell count OK. Keep `scripts/ibex-synth-capped.sh` only. |
| **uart** | golden-gap (Helion-ahead) | `SOFT-uart-ot-sv-gap-residual` | OpenTitan SV breaks Yosys/iverilog; Helion ~5704 cells. Golden SOFT only. |

Score: **PASS 97 / SOFT 3 / FAIL 0**. Gold **WNS_PS=9640**.
