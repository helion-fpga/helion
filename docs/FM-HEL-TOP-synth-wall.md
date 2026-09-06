# FM-HEL-TOP — synth / timing wall under caps

**Date:** 2026-09-06 ~03:10 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior tip:** `b18d759` (place/legalize speed)  
**Shipped:** SHA `4b14490`  
**Gold:** `WNS_PS=9640` **held**

## Goal

Profile Ibex pin-wrap/bare capped implement stages; ship cheapest honest cut to **synth** (or next biggest stage) wall under ≤120s. Never uncapped; legal fence holds.

## Stage breakdown (before, tip `b18d759`, debug caps path)

Always `IBEX_IMPL_CAP_SEC=120`; scripts prefer `target/debug/helion`.

### Pin-wrap (`examples/ibex_pin_wrap.prj`)

| Stage | ms |
|-------|-----|
| flatten | ~78 |
| **synth_rtl** | **~5362** |
| pack | ~32 |
| place (affinity+legalize) | ~229 |
| route | ~1 |
| bitgen | ~198 |
| **timing** (untimed then) | **~6780** (measured after adding hang_diag) |
| **wall** | **~13.01s** |

### Bare (`examples/ysyx_ibex.sv`)

Same shape: synth_rtl ~5306ms, wall ~13.01s, place ~231ms.

### Honesty probe inside synth_rtl (instrumented, then removed)

| Subphase | ms | notes |
|----------|-----|-------|
| aig | ~3 | |
| map (flowmap/wide) | ~223 | lut6=2354 wide=15 |
| **keep/mark_debug scan** | **~4656** | O(regs × signals × width) `bit_name` allocs |

So “synth-dominant” was almost entirely the keep/md attribute walk, not AIG/flowmap. After that cut, **timing** became the next biggest stage (~6.8s) via linear `Design::net_on` in `r2r_ps`.

## What shipped (`4b14490`)

| Change | Where |
|--------|--------|
| Prebuild `keep_bits` / `md_bits` HashSets; O(1) per reg bit | `helion-sv` `synth_rtl` |
| `r2r_ps` uses `Design::pin_index()` + static I0–I5 names | `helion-sta` |
| `hang_diag parse` / `device` / `timing … ms=` | `helion-sv`, `helion-cli` |

## Metrics (before → after), always `IBEX_IMPL_CAP_SEC=120`

### Pin-wrap

| Metric | Before `b18d759` | After `4b14490` |
|--------|------------------|-----------------|
| parse ms | (untimed) | 164 |
| synth_rtl ms | **5362** | **602** (−89%) |
| place ms | 229 | 239 |
| timing ms | ~6780 | **22** (−99%) |
| **wall** | **13.01s** | **1.40s** (−89%) |
| imux_ok / skip | 2992 / **0** | 2992 / **0** |
| iobs | **1** | **1** |
| hbits | 63807 | 63807 |
| hbits SHA-256 | `6d2914fa…f738d378` | **unchanged** |
| WNS hang_diag | 9600 | 9600 |

### Bare full

| Metric | Before | After |
|--------|--------|-------|
| synth_rtl ms | ~5306 | **601** |
| timing ms | ~6.8s | **21** |
| **wall** | **13.01s** | **1.60s** |
| imux_skip | **0** | **0** |
| iobs | **1** | **1** |
| hbits | 63279 | 63279 |
| hbits SHA | `3ae676bb…8ccf43` | **unchanged** |

### Reduced

| Metric | After |
|--------|-------|
| wall | ~0.40s |
| synth_rtl | ~69ms |
| imux_skip | **0** |
| iobs | **1** |
| hbits | 15949 |

## Regression

| Check | Result |
|-------|--------|
| Gold WNS 9640 | **held** (`project_counter_sdc_holds_gold_wns`, `project_read_ip_counter_holds_gold`) |
| `helion-cli` qor | 2/2 ok |
| `keep_attr_sets_dont_touch` | ok |
| `mark_debug_attr_on_q` | ok |
| Cap scripts | unchanged; never uncapped |
| imux_skip | **0** on reduced/pin-wrap/bare |
| IOB | **≥1** |

## Honesty caveats

- Bar move is **indexing**, not a new synth or STA algorithm. LUT INIT / r2r_ps / WNS math unchanged (identical hbits SHA + same hang_diag WNS).
- Debug binary path used by cap scripts (`target/debug/helion`); release not required for the wall bar.
- Residual wall (~1.4s pin-wrap) is mostly synth_rtl AIG/flowmap (~0.6s) + bitgen (~0.2s) + parse (~0.16s) + place (~0.24s). No further cheap win pursued this pass.
- Do **not** claim TAP DONE / board DONE / functional program from this pass.
- Never uncapped; gold 9640 held; no merge; no UNISIM/AMD IP/AXI-as-product.

## Verdict

**Bar moved:** pin-wrap wall ~13s→~1.4s and bare ~13s→~1.6s under ≤120s; synth_rtl −89%; timing −99%; imux_skip=0 / IOB≥1 / gold 9640 / hbits SHA held. Shipped `4b14490` on `fm-hel-corpus-soft-pass`.
