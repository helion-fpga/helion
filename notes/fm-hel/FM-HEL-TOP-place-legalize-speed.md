# FM-HEL-TOP — place / legalize speed under caps

**Date:** 2026-09-06 ~03:00 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior tip:** `a3da831` (.helion) / QoR baseline `90acf29`  
**Shipped:** SHA `b18d759`  
**Gold:** `WNS_PS=9640` **held**

## Goal

Cut pin-wrap/bare **place** wall under ≤120s (was ~3s, labeled “legalize”) without regressing imux_skip=0 / IOB≥1 / gold 9640. Cheap algorithmic only; never uncapped; legal fence holds.

## Honesty: where the ~3s went

Split probes on tip before affinity rewrite (same binary family, pin-wrap):

| Phase | ms |
|-------|-----|
| **affinity initial place** | **~2837** |
| bileg IMUX legalize | ~140 |
| total `hang_diag place` | ~2978 |

So the prior “legalize ~3s” was almost entirely **affinity site search**, not the 32-pass bileg loop. Legalize itself was already ~140ms after `90acf29` early-out.

## What shipped (`b18d759`)

| Change | Where |
|--------|--------|
| Free-BLE / free-column skips (`site_used_n`, `col_free`) | affinity |
| Defer die-wide `all_xs` until primary affinity/IOB columns miss | affinity |
| Per-column: affinity+base Y first, then remainder (legacy order) | affinity |
| Binary search Y in sorted column | affinity |
| Reused `xs_seen` / `y_seen` / `cand_seen` HashSets | affinity + legalize |
| O(1) `site_xy` candidate lookup | legalize |
| Fused `imux_score` (illegal+spill one walk); fused `drv_score` fanout | legalize |
| Empty-BLE: score once per site (BLE-independent; n_ble=8) | legalize |
| `hang_diag place affinity/legalize ms=` split | place |

## Metrics (before → after), always `IBEX_IMPL_CAP_SEC=120`

Scripts prefer `target/debug/helion` (unchanged). Caps untouched.

### Before (tip `a3da831` / `90acf29` QoR report)

| Design | place ms | imux_ok/skip | iobs | hbits | WNS hang_diag |
|--------|----------|--------------|------|-------|---------------|
| reduced | ~109 | 2560 / **0** | 1 | 15949 | 8420 |
| pin-wrap | **~3103** | 2992 / **0** | 1 | 63807 | 9600 |
| bare full | **~3s** | 2954 / **0** | 1 | 63279 | 8140 |

Pin-wrap hbits SHA @ `90acf29`:  
`6d2914fa7d854211ddf8afcbff00122a45d87652890d020237be7f4ef738d378`

### After (`b18d759`)

| Design | affinity ms | legalize ms | **place ms** | imux_ok/skip | iobs | hbits | WNS |
|--------|-------------|-------------|--------------|--------------|------|-------|-----|
| reduced | 18 | 3 | **21** | 2560 / **0** | 1 | 15949 | 8420 |
| pin-wrap | **89** | 140 | **230** (−93%) | 2992 / **0** | 1 | 63807 | 9600 |
| bare full | **91** | 141 | **233** (−92%) | 2954 / **0** | 1 | 63279 | 8140 |

Pin-wrap legalize move counts **identical**: sink_moved=418 sink_swapped=267 drv_moved=1 drv_swapped=2.  
Pin-wrap hbits SHA **unchanged**: `6d2914fa…f738d378`.

### Regression

| Check | Result |
|-------|--------|
| Gold counter WNS | **9640** (`project_counter_sdc_holds_gold_wns`, `project_read_ip_counter_holds_gold`) |
| `helion-cli` qor tests | 2/2 ok |
| `helion-place` lib tests | 8/8 ok |
| Cap scripts | unchanged; never uncapped |
| imux_skip | **0** on reduced/pin-wrap/bare |
| IOB | **1** |

## Honesty caveats

- Bar move is **affinity place**, not a new legalize algorithm. Legalize micro-opts are real but small vs affinity.
- Placement **decision order** preserved (per-column exhaust before next; primary xs before die-wide tail). Evidence: identical bileg move counts + identical pin-wrap SHA.
- Debug binary path used by cap scripts (`target/debug/helion`); release also rebuilt.
- Do **not** claim TAP DONE / board DONE / functional program from this pass.
- Never uncapped; gold 9640 held; no merge; no UNISIM/AMD IP/AXI-as-product.

## Verdict

**Bar moved:** pin-wrap/bare place ~3.1s→~0.23s under ≤120s; imux_skip=0 / IOB≥1 / gold 9640 / hbits SHA held. Shipped `b18d759` on `fm-hel-corpus-soft-pass`.
