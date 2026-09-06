# FM-HEL-TOP — real knight (±2,±1)/(±1,±2) IMUX + bit7 bank

**Date:** 2026-09-06 ~01:55 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `ea70415` (+ report `ac3caa3`) — pin-wrap imux_skip=**223**, full=**181**, IOB=1, gold 9640  
**Shipped:** SHA `de70989`  
**Gold:** `WNS_PS=9640` **held**

## Goal

Highest leverage honest cut to residual `imux_skip` under ≤120s via real **knight** IMUX banks (±2,±1)/(±1,±2) needing gold-stable **bit7** FeatureMap append, and/or multi-hop SB / bidirectional driver legalize — keep IOB≥1 and gold 9640. Prefer bare full skip↓ ≥20.

## What shipped

| Change | Where |
|--------|--------|
| Gold-stable **IMUX[m][7]** FeatureMap bank (append-only; abs for bits 0–6 unchanged) | `helion-device` `pack_clb`, `report_featuremap`, unit test |
| Fabric abs + 8-bit sel read + knight decode 128–191 | `helion-fabric` |
| `set_imux` 8-bit | `helion-bits` |
| Real knight sel **128–191** (8 dirs × BLE0–7) | `helion-route` `imux_sel` |
| Place: `imux_local` + `push_reach` knights; affinity Y for dx=1/2; legalize passes 14→16 | `helion-place` |
| `arch_gen` 14→15 | `devices/helion/family.had.toml` |

Encoding (0–111 unchanged from diagonal ship):

- 0–7 S±1 Q, 8–15 N±1 Q, 16–23 local Q, 24–31 local LUT O
- 32–39 S±2 Q, 40–47 N±2 Q
- 48–55 W±1 Q, 56–63 E±1 Q
- 64–71 W±2 Q, 72–79 E±2 Q
- 80–87 SW±1 Q, 88–95 SE±1 Q, 96–103 NW±1 Q, 104–111 NE±1 Q
- **128–135 W2S1, 136–143 W2N1, 144–151 E2S1, 152–159 E2N1**
- **160–167 W1S2, 168–175 W1N2, 176–183 E1S2, 184–191 E1N2** (bit7 bank; fabric samples knight-neighbor Q)

112–127 reserved. Multi-hop SB / bidirectional driver legalize **not** required for this cut.

## Metrics

### Before (`ea70415` / `ac3caa3`)

| Design | cells | iobs | imux_ok/skip | wall |
|--------|-------|------|--------------|------|
| pin-wrap | 6718 | 1 | 2769 / **223** | ~17.0s |
| bare full | 6681 | 1 | 2773 / **181** | ~15.8s |
| reduced | 2137 | 1 | — / **0** | ~1.0s |

### After — pin-wrap

```
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-pinwrap-capped.sh
```

| Metric | Value |
|--------|-------|
| cells | **6718** |
| lutffs | **4348** |
| iobs | **1** |
| legalize | moved=106 swapped=279 |
| imux_ok / imux_skip | **2861 / 131** (−92 skip vs 223) |
| hbits_bytes | **60759** |
| hbits SHA-256 | `a19f95b6e971b202b5c529a7d4ca770506b0ba609a8cbefdecf799cd3da2dfde` |
| wall | **~15.8s** ≤120s |

### After — bare full Ibex

```
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-capped.sh
```

| Metric | Value |
|--------|-------|
| cells | **6681** |
| lutffs | **4317** |
| iobs | **1** |
| legalize | moved=145 swapped=261 |
| imux_ok / imux_skip | **2881 / 73** (−108 vs 181) |
| hbits_bytes | **61083** |
| hbits SHA-256 | `10a9f6254d328928c62bd051e14054b0d19319cbdbcf115f1aa889700274527a` |
| wall | **~15.4s** ≤120s |

### Regression

| Check | Result |
|-------|--------|
| Reduced capped | exit=0, cells=2137, iobs=1, **imux_skip=0**, hbits=15949, ~1.0s |
| Gold counter WNS | **9640** |
| Fabric gold counter decode | unit test ok |
| place / route / bits / device / fabric lib tests | all ok |
| Legacy IMUX abs 0–6 | unit-tested gold-stable; bit7 at abs 1040+ |

## Honesty caveats

- **Knight (±2,±1)/(±1,±2) is real HAD interconnect** (8-bit IMUX + fabric decode of knight-neighbor Q). Not a fake mux label; legacy frame offsets for sel[4:0] / bit5 / bit6 feature abs unchanged — only new FeatureMap bank IMUX[m][7] and sel values 128–191.
- Did **not** add multi-hop SB or bidirectional driver legalize in this ship; residual skip is mostly Manhattan >2 / longer diagonals beyond knight.
- **imux_skip=131 (pin-wrap) / 73 (full) remains soft**.
- Do **not** claim TAP DONE / board DONE / functional program.
- Never uncapped; gold 9640 held; no merge; no UNISIM/AMD IP/AXI-as-product.

## What remains (cheapest next)

1. Multi-hop SB into IMUX for >±2 Manhattan / longer diagonals beyond knight.
2. Bidirectional driver legalize (move drivers toward sinks, not only sinks toward drivers) to collapse residuals onto existing reach.
3. Functional program / TAP only after IOB+IMUX are no longer soft.

## Verdict

**Bar moved:** pin-wrap imux_skip **223→131** and full **181→73** (−108 ≥20) under ≤120s via real knight IMUX + gold-stable bit7 FeatureMap bank + harder knight place legalize. IOB≥1 held. Gold 9640 held. Residual IMUX soft documented.
