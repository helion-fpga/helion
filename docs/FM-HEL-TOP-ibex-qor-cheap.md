# FM-HEL-TOP — cheap Ibex/P&R QoR under caps

**Date:** 2026-09-06 ~02:45 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior place:** SHA `fc95562` — imux_skip→0, IOB≥1, gold 9640  
**Shipped:** SHA `90acf29`  
**Gold:** `WNS_PS=9640` **held**

## Goal

Smallest honest QoR bar under ≤120s after IMUX soft closed: faster pack/place/bitgen wall, timing report on reduced/pin-wrap, honest frame counts — never uncapped; gold 9640 must hold.

## What shipped

| Change | Where |
|--------|--------|
| Sparse FeatureSet: only insert INIT/FF.USED 1-bits | `helion-bits` |
| Sparse `Bitstream::empty` (no zero-frame prealloc) | `helion-bits` |
| Early-out bileg legalize when already IMUX-legal | `helion-place` |
| `hang_diag timing WNS_PS=…` on every compile/bitstream path | `helion-cli` |

Tried densest-first column fallback for lower hbits — **no hbits move** on pin-wrap/reduced (legalize dominates), place sort tax; **reverted**. Do not thrash.

## Metrics (before → after), always `IBEX_IMPL_CAP_SEC=120`

### Before (tip pre-change, same binary family as `fc95562` metrics)

| Design | cells | iobs | imux_ok/skip | pf_iters | bitgen ms | hbits | wall |
|--------|-------|------|--------------|----------|-----------|-------|------|
| reduced | 2137 | 1 | 2560 / **0** | 1 | **141** | 15949 | ~1.0s |
| pin-wrap | 6718 | 1 | 2992 / **0** | 1 | **457** | 63807 | ~16.2s |
| bare full | 6681 | 1 | 2954 / **0** | 1 | ~(prior ~450) | 63279 | ~16s |
| counter qor | 4 | 1 | — | 1 | 13 | 185 | FRAMES=**16385** (zeros) |

### After

```
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-reduced-capped.sh
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-pinwrap-capped.sh
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-capped.sh
```

| Design | cells | iobs | imux_ok/skip | pf_iters | bitgen ms | hbits | wall | hang_diag WNS |
|--------|-------|------|--------------|----------|-----------|-------|------|---------------|
| reduced | 2137 | 1 | 2560 / **0** | 1 | **82** (−42%) | 15949 | ~1.0s | **8420** |
| pin-wrap | 6718 | 1 | 2992 / **0** | 1 | **203** (−56%) | 63807 | **~15.8s** | **9600** |
| bare full | 6681 | 1 | 2954 / **0** | 1 | **207** | 63279 | **~15.6s** | **8140** |
| counter qor | 4 | 1 | — | 1 | **0** | 185 | FRAMES=**5** | **9640** |

Pin-wrap hbits SHA-256 unchanged vs `fc95562`:  
`6d2914fa7d854211ddf8afcbff00122a45d87652890d020237be7f4ef738d378`  
(identical packets — sparse FeatureSet is encode-equivalent.)

Place reduced: ~116ms → **~109ms** (already-legal early-out). Pin-wrap place still ~3.0s (legalize required; skip not claimed).

### Regression

| Check | Result |
|-------|--------|
| Gold counter WNS | **9640** |
| `helion-cli` qor tests | 2/2 ok |
| `helion-cli` project tests | 4/4 ok |
| helion-bits / helion-place lib tests | all ok |
| Cap scripts | unchanged; never uncapped |

## Honesty caveats

- **Bitgen wall↓** is real (fewer BTreeMap inserts + no die-wide zero frames). Packet bytes / SHA unchanged on pin-wrap.
- **FRAMES** in `helion qor` now counts configured frames only (5 for counter) — prior 16385 counted reset zeros. BYTES axis unchanged.
- **hang_diag timing** exposes Ibex WNS under the same capped bitstream path; not a board timing claim. Reduced/pin-wrap/bare WNS are STA of current place/route, not gold.
- imux_skip=0 / IOB≥1 held from `fc95562`; this ship does not re-claim IMUX reach.
- Do **not** claim TAP DONE / board DONE / functional program from this QoR pass.
- Never uncapped; gold 9640 held; no merge; no UNISIM/AMD IP/AXI-as-product.

## What remains (cheapest next)

1. Lower pin-wrap/bare **place** wall (legalize still ~3s) without reintroducing imux_skip.
2. Optional denser packing that actually cuts hbits after legalize (prior densest-first did not).
3. Functional / TAP bring-up remains separate probe track.

## Verdict

**Bar moved:** Ibex capped bitgen ~2× faster, honest FRAMES, and timing WNS printed on reduced/pin-wrap/bare under ≤120s. hbits bytes unchanged (SHA stable). Gold 9640 held. No thrash after densest-first no-op.
