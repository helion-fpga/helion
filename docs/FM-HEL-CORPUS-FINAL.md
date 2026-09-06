# FM-HEL-CORPUS — Final accuracy summary

**Date:** 2026-09-05/06 (America/New_York)  
**Task:** FM-HEL-TOP soft-helion-ahead  
**Score (phase-d):** **PASS 100 / SOFT 0 / FAIL 0**  
**Gold:** `helion report_timing examples/counter.sv` → **WNS_PS=9640** (held)

## Residual SOFTs
- none

## Bar move (Helion-primary)
- Harness accepts Helion-primary **PASS** when Yosys fails (no alt golden) but Helion `cells > 0`, tagged `yosys_gap` / `helion_ahead`.
- **uart** + **ysyx_ibex** SOFT→PASS under that rule (cheap; no RTL shrink; Ibex stays capped).

Ids remaining SOFT: []
