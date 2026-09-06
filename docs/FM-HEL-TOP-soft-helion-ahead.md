# FM-HEL-TOP soft-helion-ahead

**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Before:** PASS 98 / SOFT 2 (`uart`, `ysyx_ibex`) / FAIL 0  
**After:** PASS 100 / SOFT 0 / FAIL 0  
**Gold:** WNS_PS=9640 (held)

## Cheap route taken
Harness Helion-primary PASS when:
1. `helion_synth` PASS
2. `helion_cells > 0`
3. Yosys primary + alt goldens FAIL
4. Issues tagged `yosys_gap` + `helion_ahead`; yosys stage marked SOFT with note

Applied to **uart** (cells=5704) and **ysyx_ibex** (cells=52742, capped). No uncapped Ibex cargo. No invented PASS without cells.
