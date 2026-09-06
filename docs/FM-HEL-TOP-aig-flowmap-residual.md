# FM-HEL-TOP — AIG/flowmap residual after `4b14490`

**Date:** 2026-09-06 ~03:10 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Tip:** `4b14490` (unchanged — **not shipped**)  
**Gold:** `WNS_PS=9640` **held**

## Goal

One honest probe of pin-wrap residual ~1.4s after synth/timing indexing (`4b14490`). Ship only a **cheap** cut (≥15% wall or clear stage ms↓) without QoR regress. Else stop: residual is AIG/flowmap-dominated; no thrash. Plus one Mac FTDI/OFL retry for board DONE (never fake).

## Stage breakdown (tip `4b14490`, debug caps path)

Always `IBEX_IMPL_CAP_SEC=120`; `target/debug/helion` bitstream pin-wrap.

| Stage | ms | notes |
|-------|-----|-------|
| parse | **163** | 97 mods / ~1.06 MB / 2 files |
| flatten | 78 | |
| **synth_rtl** | **621** | keep/md already O(1); residual = AIG + flowmap + cell build |
| pack | 33 | lutffs=4348 iobs=**1** |
| place affinity | 93 | |
| place legalize | 141 | sink_moved=418 sink_swapped=267 (unchanged) |
| place total | 235 | |
| route | 2 | imux=2992 **imux_skip=0** |
| **bitgen** | **207** | bytes=63807 (already sparse) |
| timing | 20 | WNS hang_diag=9600 |
| **wall** | **1.40s** | exit=0 |

hbits SHA-256: `6d2914fa7d854211ddf8afcbff00122a45d87652890d020237be7f4ef738d378` (**unchanged** vs `4b14490` / `b18d759` / `90acf29`).

Share of residual wall (largest):

1. synth_rtl ~44% (AIG `from_expr` + `flowmap_lut6` / wide LUT2 trees per reg/comb cone)
2. place ~17% (already affinity+legalize micro-opt shipped)
3. bitgen ~15% (sparse encoder already shipped)
4. parse ~12%

≥15% wall bar ≈ **≥210 ms**. No remaining “indexing bug” class win in keep/md or STA pin index.

## Cheap-cut probe (AIG/flowmap)

Inspected `Aig::{from_expr,flowmap_lut6,eval_lit_memo}` and `synth_rtl` map loop:

| Idea | Why not cheap / this pass |
|------|---------------------------|
| HashMap PI names in `pi()` | PI count ≤6 on flowmap path — noise |
| Bit-parallel / reuse memo across `flowmap_lut6` addrs | Real algo rewrite; QoR/INIT risk; not ≥15% wall without deeper work |
| Skip more comb | Already `skip_comb_assigns n=1336` on Ibex hang path |
| Parse / bitgen / place further | Parse is sv-parser; bitgen sparse; place already −93% |

**Verdict on wall:** **no cheap ≥15% cut**. Residual is honest AIG/flowmap (+ bitgen/parse tails). **Stop thrash. No ship.**

## Holds (re-measured)

| Check | Result |
|-------|--------|
| imux_skip | **0** |
| IOB | **1** |
| gold counter WNS | **9640** (`helion report_timing examples/counter.sv --sdc examples/counter.sdc`) |
| pin-wrap hbits SHA | **held** `6d2914fa…f738d378` |
| Cap | ≤120s; never uncapped |
| Merge | **none** |

## Mac FTDI / OFL retry (board DONE)

| Check | Result |
|-------|--------|
| `ListMachines` | Mac `e6c67522-8627-418d-a593-df7fc43c59db` `connected: true` |
| Shell via machineId | **temporarily unreachable** (spawn error) — twice this turn; same class as prior live-FTDI pass |
| Box USB | **0** devices (`openFPGALoader --scan-usb` → “No USB devices found”) |
| Box OFL | `/usr/bin/openFPGALoader` present; `--detect` → `unable to open ftdi device: -3 (device not found)` / JTAG init failed |
| Board / TAP DONE | **not claimed** — no probe evidence |

**Honesty:** No board DONE. Mac local-exec unreachable despite ListMachines “connected”; box OFL proves **no-device** path only.

## What was not done

- No AIG/flowmap algorithm rewrite
- No commit/push on `helion` (bar did not move)
- No merge / no force-push / no uncapped Ibex
- No invented STAT / fake DONE

## What remains (out of this residual)

1. When Mac local-exec works: USB profile + OFL/`--cable native` on real FTDI; claim DONE only on live STAT TDO DONE=1
2. Any further wall↓ needs non-cheap AIG/flowmap (or release binary / batching) — not this pass

## Verdict

**STOP — residual AIG/flowmap dominated.** Pin-wrap wall **1.40s** unchanged at tip `4b14490`; no ship; imux_skip=0 / IOB=1 / gold 9640 / hbits SHA held. Mac FTDI unreachable; box OFL **no device**. No board DONE. No merge.
