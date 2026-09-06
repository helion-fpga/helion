# FM-HEL-TOP — Ibex IOB + IMUX residual

**Date:** 2026-09-06 ~01:25 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `6930898` — full `ysyx_ibex` under ≤120s, **IOB=0**, imux_skip=580  
**Gold:** `WNS_PS=9640` **held**

## Goal

Highest leverage, smallest honest ship:
1. Pin-reduced full-core wrap with **≥1 real IOB**, still ≤120s, **and/or**
2. Longer-reach IMUX so `imux_skip` drops meaningfully under the same cap.

## What shipped

| Change | Where |
|--------|--------|
| `examples/ibex_pin_wrap.sv` + `.prj` | Full `ysyx_ibex` child + clk/led heartbeat FF |
| `helion bitstream` accepts `.prj` | `helion-cli` `synth_any` |
| IMUX place legalization (≤3 passes) | `helion-place` — pull sinks onto driver same-CLB / N-S±1 |
| Cap scripts | `scripts/ibex-impl-pinwrap-capped.{sh,py}` (PG SIGKILL, never uncapped) |

HAD IMUX remains same-CLB / N-S±1 only (sel 0–23); no fake E-W encoding.

## Metrics

### Before (`6930898`, full `ysyx_ibex`)

| Metric | Value |
|--------|-------|
| cells | 6930 |
| lutffs | 4317 |
| iob_trim | 250 → **0** (driven=0) |
| imux_ok / imux_skip | 2374 / **580** |
| hbits | 54693 |
| wall | ~15.8s |

### After — pin-wrap (primary bar)

```
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-pinwrap-capped.sh
```

| Metric | Value |
|--------|-------|
| top | `ibex_pin_wrap` (clk + led) |
| cells (synth) | **6718** (~5.2s; −250 AXI IobOut vs bare top, +heartbeat) |
| lutffs | **4348** |
| **iobs** | **1** (FF-driven `led`; PathFinder iters=1, overused=0) |
| place legalize moved | 8 |
| imux_ok / imux_skip | **2426 / 566** |
| hbits_bytes | **55439** |
| hbits SHA-256 | `bf3a1cd54c5ee297c7ece74947c481bc3511fe50842d078b2e1d7e08261fbaf4` |
| wall | **~15.8s** ≤120s |

### After — bare full Ibex (legalize only)

| Metric | Value |
|--------|-------|
| cells | 6930 |
| iob_trim | still **0** (AXI outs undriven under skip_comb_assigns) |
| legalize moved | 14 |
| imux_ok / imux_skip | 2388 / **566** (580→566) |
| hbits | 54879 |
| wall | ~15.8s |

### Regression

| Check | Result |
|-------|--------|
| Reduced capped | exit=0, cells=2137, iobs=1, imux_skip=0, hbits=15949, ~1.0s |
| Gold WNS | **9640** |
| `helion-cli` qor | 2/2 ok |

## Honesty caveats

- **Pin-wrap IOB=1 is real** (board-facing `led` from heartbeat FF + PathFinder route). Not a functional Ibex/SoC bring-up; AXI/SRAM tied off; `skip_comb_assigns` still on.
- **Bare `ysyx_ibex` still IOB=0** after trim — wrap is the honest pin path.
- **imux_skip=566** remains soft (bring-up IMUX N-S/same-CLB only; legalize helped −14). Do **not** claim TAP DONE / board DONE / functional program.
- Never uncapped; gold 9640 held; no merge; no UNISIM/AMD IP/AXI-as-product.

## What remains (cheapest next)

1. Stronger IMUX legalization / cluster place (or real longer-reach interconnect in HAD) to drive `imux_skip→0`.
2. Optional: stop emitting undriven AXI IobOut when `skip_comb_assigns` so bare top can fall back to one FF→PAD without a wrap.
3. Functional program / TAP only after IOB+IMUX are no longer soft.

## Verdict

**Bar moved:** pin-reduced full-core wrap writes `.hbits` in ~16s with **IOB≥1** and slightly lower `imux_skip`. Gold held. Residual IMUX soft documented.
