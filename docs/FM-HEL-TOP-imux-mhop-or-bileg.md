# FM-HEL-TOP — bidirectional IMUX legalize + axis±3/±4 / diag±2 reach

**Date:** 2026-09-06 ~02:10 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `de70989` (+ stamp `e7452c3`) — pin-wrap imux_skip=**131**, full=**73**, IOB=1, gold 9640  
**Shipped:** SHA `PENDING`  
**Gold:** `WNS_PS=9640` **held**

## Goal

Highest leverage honest cut of residual `imux_skip` under ≤120s via **bidirectional driver legalize** and/or multi-hop SB — prefer bare full skip↓ ≥20 toward soft→harder; Firstmate bar raised to **imux_skip→0** on bare + pin-wrap if honestly possible. Keep IOB≥1, reduced skip=0, gold 9640.

## What shipped

| Change | Where |
|--------|--------|
| Bidirectional IMUX legalize (sink→driver + driver→sink) with fanout-primary scoring | `helion-place` |
| Spill-distance + gradient walk (step toward far endpoints across passes) | `helion-place` `imux_spill` / `push_gradient` |
| Real N-S ±3 (sel **112–127**, was reserved) | `helion-route` / `helion-fabric` |
| Real E-W ±3 (sel **192–207**) | route + fabric |
| Real diagonal ±2 (sel **208–239**) | route + fabric |
| Real N-S ±4 (sel **240–255**) | route + fabric |
| `imux_local` + `push_reach` + affinity for new reach; legalize passes 16→32 | `helion-place` |
| `arch_gen` 15→17 | `devices/helion/family.had.toml` |

Encoding (0–111 + knight 128–191 unchanged):

- **112–119 S±3 Q, 120–127 N±3 Q** (fills prior reserved bank)
- 128–191 knight (unchanged)
- **192–199 W±3 Q, 200–207 E±3 Q**
- **208–215 SW2, 216–223 SE2, 224–231 NW2, 232–239 NE2**
- **240–247 S±4 Q, 248–255 N±4 Q**

No new FeatureMap bank — all new sels fit existing 8-bit IMUX (bit6/bit7). Legacy abs for bits 0–7 unchanged. True multi-hop SB **not** required: residual after knight was long N-S that bileg gradient collapsed onto short arcs, then extended neighbor Q closed them.

## Metrics

### Before (`de70989`)

| Design | cells | iobs | imux_ok/skip | wall |
|--------|-------|------|--------------|------|
| pin-wrap | 6718 | 1 | 2861 / **131** | ~15.8s |
| bare full | 6681 | 1 | 2881 / **73** | ~15.4s |
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
| legalize | sink_moved=418 sink_swapped=267 drv_moved=1 drv_swapped=2 |
| imux_ok / imux_skip | **2992 / 0** (−131 skip) |
| hbits_bytes | **63807** |
| hbits SHA-256 | `6d2914fa7d854211ddf8afcbff00122a45d87652890d020237be7f4ef738d378` |
| wall | **~17.8s** ≤120s |

### After — bare full Ibex

```
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-capped.sh
```

| Metric | Value |
|--------|-------|
| cells | **6681** |
| lutffs | **4317** |
| iobs | **1** |
| legalize | sink_moved=355 sink_swapped=263 drv_moved=0 drv_swapped=0 |
| imux_ok / imux_skip | **2954 / 0** (−73 skip) |
| hbits_bytes | **63279** |
| hbits SHA-256 | `3ae676bb596fb452db2f5dd79455ac9dd9fc1de1b92c51b02ad767366d8ccf43` |
| wall | **~16.0s** ≤120s |

### Regression

| Check | Result |
|-------|--------|
| Reduced capped | exit=0, cells=2137, iobs=1, **imux_skip=0**, hbits=15949, ~1.0s |
| Gold counter WNS | **9640** (`helion project examples/counter.prj`) |
| Fabric gold counter decode | unit test ok |
| place / route / bits / device / fabric lib tests | all ok |
| Legacy IMUX abs 0–7 | unchanged (no new FeatureMap bank) |

## Honesty caveats

- **Bidirectional + spill/gradient legalize** is real place: walks sinks/drivers toward each other onto existing/extended HAD reach — not a fake PASS.
- **Axis ±3/±4 and diag ±2** are the same honesty class as prior ±2 / knight neighbor-Q IMUX (fabric `q_at` at offset sites). Not multi-hop SB bitstream; no SB FeatureMap invented.
- True **multi-hop SB** (programmed switchbox tracks into IMUX) was **not** needed for skip→0 on these designs and remains future interconnect work if longer arcs reappear.
- Do **not** claim TAP DONE / board DONE / functional program.
- Never uncapped; gold 9640 held; no merge; no UNISIM/AMD IP/AXI-as-product.

## What remains (cheapest next)

1. Functional program / TAP / board bring-up now that IOB+IMUX are no longer soft on Ibex bare/pin-wrap/reduced.
2. Optional: real multi-hop SB FeatureMap if future designs reintroduce >±4 Manhattan soft arcs.
3. Keep watching gold WNS 9640 on any further FeatureMap append.

## Verdict

**Bar moved to hard close on IMUX soft:** pin-wrap imux_skip **131→0** and bare full **73→0** under ≤120s via bidirectional+gradient place legalize plus real axis±3/±4 and diag±2 IMUX (8-bit sel fill). IOB≥1 held. Reduced skip=0 held. Gold 9640 held.
