# FM-HEL-TOP — implement quality (verify, no AIG thrash)

**Date:** 2026-09-06 ~08:25 America/New_York (EDT)  
**Branch:** `fm-hel-top` — PR https://github.com/helion-fpga/helion/pull/8 (**NO MERGE**)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**Fence:** AIG/flowmap **STOP** (no thrash). No uncapped Ibex. Sim absolute split / `SPLITTER_GRAB_PX` / void Timing-Reports **untouched**.

## Gold (counter)

```
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

Verified this turn (box):

```
report_timing counter WNS_PS=9640 TNS_PS=0 endpoints=4 r2r_ps=360 iob_ps=220
```

Log: `/workspace/reports/gold-counter-timing.txt`

## Reduced Ibex (capped ≤120s)

```
IBEX_IMPL_CAP_SEC=120 bash scripts/ibex-impl-reduced-capped.sh
```

| Metric | Value |
|--------|-------|
| cells | **2137** |
| lutffs | 1112 |
| iobs | 1 |
| imux | 2560 |
| **imux_skip** | **0** |
| pathfinder_iters | 1 |
| hbits_bytes | 15949 |
| hang_diag WNS_PS | 8420 |
| **wall** | **0.40s** (cap 120s; exit 0) |

Log: `/workspace/reports/ibex-impl-reduced-capped.txt`  
Also mirrored under `docs/` reports path via `/workspace/reports/`.

## Air residual

```
rg 'request_repaint\(' crates/helion-gui/src
→ empty call sites (policy/comments only)
```

`IDLE_PAINT_POLICY` = `reactive-no-request_repaint` (`crates/helion-gui/src/chrome.rs`).  
IDE `update`: reactive eframe only — never `request_repaint` / Continuous.

## Board UX soft-hold crumb

When `Activity::Program` and `detect_boards()` shows `!physical_had`, status bar appends monospace crumb **`board:soft-hold`** (visible without opening Program side). Sim Program still works. No fake DONE.

## Verdict

**PASS (verify).** Gold **9640**; reduced cells=2137 imux_skip=0 wall=0.40s; air idle-clean; soft-hold crumb shipped. **NO MERGE.** AIG STOP held.
