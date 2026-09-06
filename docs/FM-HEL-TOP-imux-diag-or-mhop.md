# FM-HEL-TOP — real diagonal (±1,±1) IMUX + harder diag place

**Date:** 2026-09-06 ~01:46 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `aed5bb5` — pin-wrap imux_skip=**277**, full=**245**, IOB=1, gold 9640  
**Shipped:** SHA `ea70415`  
**Gold:** `WNS_PS=9640` **held**

## Goal

Highest leverage honest cut to residual `imux_skip` under ≤120s via real **diagonal** IMUX banks (N/S±1 with E/W±1 neighbor Q) and/or multi-hop SB / bidirectional driver legalize — keep IOB≥1 and gold 9640. Prefer measurable skip↓ on bare full (≥20).

## What shipped

| Change | Where |
|--------|--------|
| Real diagonal (±1,±1) IMUX sel **80–111** (SW/SE/NW/NE × BLE0–7) in remaining 7-bit space | `helion-route` `imux_sel`, `helion-fabric` `decode_imux` |
| Place: `imux_local` + `push_reach` diagonals; affinity Y for dx=1 cols; legalize passes 12→14 | `helion-place` |
| Comments: bit6 bank also covers diag encodings (no new FeatureMap bank; legacy abs unchanged) | `helion-bits`, `helion-device` |
| `arch_gen` 13→14 | `devices/helion/family.had.toml` |

Encoding (0–79 unchanged from E-W±2 ship):

- 0–7 S±1 Q, 8–15 N±1 Q, 16–23 local Q, 24–31 local LUT O
- 32–39 S±2 Q, 40–47 N±2 Q
- 48–55 W±1 Q, 56–63 E±1 Q
- 64–71 W±2 Q, 72–79 E±2 Q
- **80–87 SW±1 Q, 88–95 SE±1 Q, 96–103 NW±1 Q, 104–111 NE±1 Q** (7-bit sel; fabric samples Q at (x±1,y±1))

No bit7 FeatureMap bank — diagonals reuse existing IMUX[m][6] / bit5 packing (sel binary). Gold frames for sel&lt;32 unchanged.

Multi-hop SB not required for this cut; residual &gt;±2 / longer diagonal still soft.

## Metrics

### Before (`aed5bb5`)

| Design | cells | iobs | imux_ok/skip | wall |
|--------|-------|------|--------------|------|
| pin-wrap | 6718 | 1 | 2715 / **277** | ~15.8s |
| bare full | 6681 | 1 | 2709 / **245** | ~15.6s |
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
| legalize | moved=36 swapped=244 |
| imux_ok / imux_skip | **2769 / 223** (−54 skip vs 277) |
| hbits_bytes | **58881** |
| hbits SHA-256 | `177eb824de3c1c8c460758ae4e984fdfe0e0cf2f5f1a243b36bce8b0a3c16a54` |
| wall | **~17.0s** ≤120s |

### After — bare full Ibex

```
IBEX_IMPL_CAP_SEC=120 ./scripts/ibex-impl-capped.sh
```

| Metric | Value |
|--------|-------|
| cells | **6681** |
| lutffs | **4317** |
| iobs | **1** |
| legalize | moved=67 swapped=272 |
| imux_ok / imux_skip | **2773 / 181** (−64 vs 245) |
| hbits_bytes | **58855** |
| hbits SHA-256 | `afda5c62566729f633416909e61c16f2ba8aa6e42b745aad5ac0075a9d6b0760` |
| wall | **~15.8s** ≤120s |

### Regression

| Check | Result |
|-------|--------|
| Reduced capped | exit=0, cells=2137, iobs=1, **imux_skip=0**, hbits=15949, ~1.0s |
| Gold counter WNS | **9640** |
| Fabric gold counter decode | unit test ok |
| place / route / bits / device / fabric lib tests | all ok |

## Honesty caveats

- **Diagonal (±1,±1) is real HAD interconnect** (7-bit IMUX + fabric decode of neighbor-column×neighbor-row Q). Not a fake mux label; legacy frame offsets for sel[4:0] / bit5 / bit6 feature abs unchanged — only new sel values 80–111.
- Did **not** add multi-hop SB or bit7 (±2,±1)/(±1,±2) in this ship; residual skip is mostly longer Manhattan / knight-move diagonals.
- **imux_skip=223 (pin-wrap) / 181 (full) remains soft**.
- Do **not** claim TAP DONE / board DONE / functional program.
- Never uncapped; gold 9640 held; no merge; no UNISIM/AMD IP/AXI-as-product.

## What remains (cheapest next)

1. Longer diagonal / knight reach (dx=±2,dy=±1 / dx=±1,dy=±2) via **bit7** sel banks (same append pattern as bit5/bit6) **or** multi-hop SB into IMUX for &gt;±2 Manhattan.
2. Bidirectional driver legalize (move drivers toward sinks, not only sinks toward drivers) to collapse residuals onto existing reach.
3. Functional program / TAP only after IOB+IMUX are no longer soft.

## Verdict

**Bar moved:** pin-wrap imux_skip **277→223** and full **245→181** (−64 ≥20) under ≤120s via real diagonal (±1,±1) IMUX + harder diag place legalize. IOB≥1 held. Gold 9640 held. Residual IMUX soft documented.
