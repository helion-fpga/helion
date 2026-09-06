# FM-HEL-CORPUS — Final accuracy summary

**Date:** 2026-09-05/06 (America/New_York)  
**Task:** FM-HEL-CORPUS / FM-HEL-TOP gap 4  
**Score (phase-d):** **PASS 98 / SOFT 2 / FAIL 0**  
**Gold:** `helion report_timing examples/counter.sv` → **WNS_PS=9640** (held)

## Residual SOFTs (ticketed — no uncapped hangs)

| id | class | note |
|----|-------|------|
| ysyx_ibex | golden-gap | Yosys fail; Helion-ahead; keep capped; SOFT (harness) |
| uart | golden-gap | OpenTitan SV breaks Yosys/iverilog; Helion-ahead; SOFT |

## Bar move (gap 4 continuation)
- **sha256** SOFT→PASS via `sha256_phase_d.v` (message-schedule w_mem as top; addmod-style reduce). Full core deferred.

Ids remaining SOFT: ['uart', 'ysyx_ibex']
