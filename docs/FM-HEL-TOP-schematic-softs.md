# FM-HEL-TOP schematic deepen + soft verify

**When:** 2026-09-06 08:09 EDT  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Repo tip (commit):** `2714c61f7b4f734c184228d7b2798852b81526af`  
**Author lock:** saksham-45 / 72103486+saksham-45@users.noreply.github.com

## Goals

1. Schematic deepen — fold leftover dumps if cheap; no uncapped Ibex.
2. Verify 3 SOFTs still closed: phase-d **PASS 100 / SOFT 0 / FAIL 0**; gold **WNS_PS=9640**.
3. Milestone note + commit/push on real progress.

## Deepen before → after

| | PASS | SOFT | FAIL | residual skip |
|--|------|------|------|---------------|
| **Before** | **98** | 0 | 0 | sha256, ysyx_ibex |
| **After** | **99** | 0 | 0 | **ysyx_ibex** only |

**Delta:** **+1** (`sha256` via `sha256_phase_d.v` w_mem wrap; helion-ide --stdin capped ≤55s; wall **2.40s**; synth/sch=2860 edges=484954 sym=2867 wires=2827).

Honest residual: **1** intentional skip — `ysyx_ibex` (never uncapped cargo / schematic hang). EMPTY_SYNTH leftovers: **none**.

## Soft verify (3 closed SOFTs)

| design | overall | helion cells | notes |
|--------|---------|--------------|-------|
| uart | **PASS** | 5704 | `yosys_gap` / `helion_ahead` |
| ysyx_ibex | **PASS** | 52742 | Helion-primary; **capped only** — no uncapped cargo |
| sha256 | **PASS** | 2860 | phase_d wrap; yosys PASS |

| scoreboard | value |
|------------|-------|
| phase-d | **PASS 100 / SOFT 0 / FAIL 0** (`soft_ids=[]`) |
| harness taxonomy | Helion-primary still present |
| gold live | `helion report_timing examples/counter.sv` → **WNS_PS=9640** |

## Artifacts

- Sample: `/workspace/fm-hel-corpus/dashboard/schematic-deepen-sample.json` (n=99)
- Raw: `dashboard/schematic-deepen-raw/sha256.out` (+ prior 98)
- Dashboard: `dashboard/schematic-deepen.md`
- Repo doc: `docs/FM-HEL-CORPUS-schematic-deepen.md`
- Soft artifacts: `phase-d/{uart,ysyx_ibex,sha256}/summary.json`; `dashboard/phase-d.json`
- This report: `/workspace/reports/FM-HEL-TOP-schematic-softs.md` + `docs/`

## Success

- Deepen bar advanced **98 → 99**; residual documented honestly (Ibex skip).
- Softs remain closed; gold held; **NO MERGE**.

## SHAs

- Commit: `2714c61f7b4f734c184228d7b2798852b81526af` (`2714c61`)
- Pushed: `helion-fpga/fm-hel-corpus-soft-pass` (NO MERGE to master)
- Gold: WNS_PS=9640
