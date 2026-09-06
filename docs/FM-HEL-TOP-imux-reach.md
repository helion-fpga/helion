# FM-HEL-TOP — IMUX reach (N-S±2 + stronger legalize)

**Date:** 2026-09-06 ~01:35 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `c4ad720` — pin-wrap IOB=1, imux_skip=**566**, gold 9640  
**Gold:** `WNS_PS=9640` **held**

## Goal

Cut `imux_skip` meaningfully on pin-wrap and/or full Ibex under ≤120s PG-kill caps via **real** longer-reach HAD IMUX and/or stronger legalization — not fake E-W encodings.

## What shipped

| Change | Where |
|--------|--------|
| Real N-S **±2** IMUX (sel 32–39 S±2, 40–47 N±2) | `helion-route` `imux_sel`, `helion-fabric` `decode_imux` |
| IMUX bit5 extension bank (gold-stable abs for bits 0–4) | `helion-device` FeatureMap, `helion-bits` `set_imux`, fabric `abs_feature` |
| Stronger place legalize: ±2 candidates, empty-BLE move, pairwise swap, ≤8 passes | `helion-place` |
| Affinity Y order includes ±2 | `helion-place` initial site pick |
| `arch_gen` 10→11 | `devices/helion/family.had.toml` |

Encoding (unchanged 0–31; extension uses bit5 appended after 64×5 so legacy frame offsets for sel[4:0] stay gold-compatible):

- 0–7 S±1 Q, 8–15 N±1 Q, 16–23 local Q, 24–31 local LUT O  
- **32–39 S±2 Q, 40–47 N±2 Q**  
- No E-W IMUX (would be dishonest without wire/SB path)

## Metrics

### Before (`c4ad720` pin-wrap)

| Metric | Value |
|--------|-------|
| cells | 6718 |
| lutffs | 4348 |
| iobs | 1 |
| imux_ok / imux_skip | 2426 / **566** |
| hbits | 55439 |
| wall | ~15.8s |
| legalize | moved=8 |

### After — pin-wrap (primary)

```
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-pinwrap-capped.sh
```

| Metric | Value |
|--------|-------|
| cells | **6718** |
| lutffs | **4348** |
| iobs | **1** |
| legalize | moved=4 swapped=196 |
| imux_ok / imux_skip | **2625 / 367** (−199 skip) |
| hbits_bytes | **56927** |
| hbits SHA-256 | `696bb4ec58e63a75bf58a9e06ce7de93d1069b587ddf81a2790a6e1716b8003a` |
| wall | **~15.8s** ≤120s |

### After — bare full Ibex

| Metric | Value |
|--------|-------|
| cells | 6930 |
| iob_trim | still **0** |
| legalize | moved=19 swapped=200 |
| imux_ok / imux_skip | **2603 / 351** (was 566) |
| hbits | 56479 |
| wall | ~16.0s |

### Regression

| Check | Result |
|-------|--------|
| Reduced capped | exit=0, cells=2137, iobs=1, **imux_skip=0**, hbits=15949, ~1.0s |
| Gold counter WNS | **9640** |
| Fabric gold counter decode | unit test ok |
| place/route/device/fabric lib tests | all ok |

## Honesty caveats

- **N-S±2 is real HAD interconnect** (bit5 extension + fabric decode + route programming). Not a fake E-W mux.
- **imux_skip=367 (pin-wrap) / 351 (full) remains soft** — residual is mostly E-W / >±2 / diagonal FF→LUT; still no E-W IMUX.
- Pin-wrap **IOB=1** unchanged (heartbeat led). Bare top still IOB=0 after trim.
- Do **not** claim TAP DONE / board DONE / functional program.
- Never uncapped; gold 9640 held; no merge; no UNISIM/AMD IP/AXI-as-product.

## What remains (cheapest next)

1. Real E-W / longer SB hop interconnect in HAD (uwilton) **or** cluster place that collapses E-W affinity onto same column harder.
2. Optional: stop emitting undriven AXI IobOut under `skip_comb_assigns` so bare top can keep one FF→PAD without wrap.
3. Functional program / TAP only after IOB+IMUX are no longer soft.

## Verdict

**Bar moved:** pin-wrap imux_skip **566→367** and full **566→351** under ≤120s via real N-S±2 IMUX + swap legalize. Gold held. Residual IMUX soft documented.
