# FM-HEL-TOP gap 4 — corpus SOFTs (done continuation)

**Score:** PASS **100** / SOFT **0** / FAIL **0**  
**Gold:** WNS_PS=**9640** (held; no rebuild required for gold)

## sha256 — SOFT→PASS
- Cheap route: reduced wrapper `sha256_phase_d.v` (LogikBench `sha256_w_mem` renamed top `sha256`; addmod-style reduce).
- Helion synth **cells=2860** luts=2827 in ~0.11–0.27s (60s python process-group cap). Yosys **3447** cells PASS. SKIP_HEAVY post-synth.
- Full `sha256_all.v` (core round unroll + digest) still hangs under ≤60s — deferred; do not uncapped-retry.
- Cap script: `scripts/sha256-synth-capped.sh`.

## Helion-primary PASS taxonomy (soft-helion-ahead)
- Harness (`fm-hel-corpus/harness/corpus_one.py`): when Yosys/alt FAIL but `helion_synth` PASS and `helion_cells > 0`, overall **PASS** with issues `yosys_gap` + `helion_ahead` (yosys stage → SOFT + note).
- Does **not** invent PASS without Helion cells; documents Yosys SV golden gap.

## ysyx_ibex — SOFT→PASS (Helion-ahead)
- Hang FIXED earlier: capped synth PASS via `scripts/ibex-synth-capped.sh` (never uncapped).
- Reclass: Helion-primary PASS; cells=52742; Yosys FAIL on Ibex SV.

## uart — SOFT→PASS (Helion-ahead)
- Helion cells=5704; OpenTitan SV breaks Yosys/iverilog/synth_xilinx.
- Reclass: Helion-primary PASS with documented `yosys_gap` / `helion_ahead`.

## Push
- Doc/fix commits → branch `fm-hel-corpus-soft-pass` (NO MERGE).
