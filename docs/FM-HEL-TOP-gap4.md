# FM-HEL-TOP gap 4 — corpus SOFTs (done continuation)

**Score:** PASS **98** / SOFT **2** / FAIL **0**  
**Gold:** WNS_PS=**9640** (held; no rebuild required for gold)

## sha256 — SOFT→PASS
- Cheap route: reduced wrapper `sha256_phase_d.v` (LogikBench `sha256_w_mem` renamed top `sha256`; addmod-style reduce).
- Helion synth **cells=2860** luts=2827 in ~0.11–0.27s (60s python process-group cap). Yosys **3447** cells PASS. SKIP_HEAVY post-synth.
- Full `sha256_all.v` (core round unroll + digest) still hangs under ≤60s — deferred; do not uncapped-retry.
- Cap script: `scripts/sha256-synth-capped.sh`.

## ysyx_ibex — SOFT (Helion-ahead, hang fixed)
- Hang FIXED earlier: capped synth PASS (~6930 cells) via `scripts/ibex-synth-capped.sh`.
- Harness taxonomy: Yosys FAIL with no alt golden → overall **SOFT** (`yosys_fail`). Only existing Helion-ahead PASS pattern is `hamming` (yosys SOFT + iverilog/synth_xilinx alt when helion cells>0). **Did not invent** Helion-primary PASS.

## uart — SOFT (Helion-ahead)
- Helion ~5704 cells; OpenTitan SV breaks Yosys/iverilog.
- Same taxonomy — leave SOFT; document `yosys_gap` / `helion_ahead`. No FAIL invented.

## Push
- Doc/fix commits → branch `fm-hel-corpus-soft-pass` (NO MERGE).
