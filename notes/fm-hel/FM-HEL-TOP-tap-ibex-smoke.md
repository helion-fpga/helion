# FM-HEL-TOP — Ibex/pin-wrap mpsse-sim STAT smoke + OFL/HAD honesty

**Date:** 2026-09-06 ~02:10 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior TAP:** SHA `b13b3f3` — counter.hbits CFG_W + STAT DONE via `--cable mpsse-sim`  
**Prior place:** SHA `fc95562` — imux_skip→0, IOB=1, gold 9640  
**Shipped:** SHA `9ef8bdc`  
**Gold:** `WNS_PS=9640` **held** (`helion report_timing examples/counter.sv --sdc examples/counter.sdc`)

## Goal

Highest leverage honest next step toward board DONE after sim CFG_W:
1. Smoke pin-wrap / reduced / bare Ibex `.hbits` under ≤120s caps, then `helion-prog --cable mpsse-sim` and prove **STAT DONE=1** (sizes/wall).
2. Cheap OFL/HAD honesty (never invent Helion TAP STAT; native stays NotImplemented).

## What shipped

| Change | Where |
|--------|--------|
| Capped Ibex mpsse-sim smoke (`reduced`/`pinwrap`/`bare`) | `scripts/ibex-prog-mpsse-sim-smoke.{sh,py}` |
| Detect / HAD notes: OFL `TAP_readback=none`; mpsse-sim listed; native NotImplemented until real FTDI MPSSE | `helion-hw` `lib.rs` |

Never uncapped. Cap scripts unchanged (default 120s). No place/route redo.

## Build evidence (≤120s caps)

| Design | Script | cells | lutffs | iobs | imux_ok/skip | hbits | SHA-256 | wall |
|--------|--------|-------|--------|------|--------------|-------|---------|------|
| reduced | `ibex-impl-reduced-capped.sh` | 2137 | 1112 | 1 | 2560 / **0** | **15949** | `9bfd0dbb…5f63ad` | **~1.00s** |
| pin-wrap | `ibex-impl-pinwrap-capped.sh` | 6718 | 4348 | 1 | 2992 / **0** | **63807** | `6d2914fa…38d378` | **~16.01s** |
| bare full | `ibex-impl-capped.sh` | 6681 | 4317 | 1 | 2954 / **0** | **63279** | `3ae676bb…ccf43` | **~15.61s** |

All exit=0 under `IBEX_IMPL_CAP_SEC=120`. No timeout.

## Program evidence (`--cable mpsse-sim`)

```
IBEX_SMOKE_SKIP_BUILD=1 ./scripts/ibex-prog-mpsse-sim-smoke.sh
```

| Design | bytes | frames | STAT | prog_wall | exit |
|--------|-------|--------|------|-----------|------|
| reduced | 15949 | 909 | INIT=1 **DONE=1** EOS=1 GWE=1 CRC_ERR=0 | ~0.021s | 0 |
| pin-wrap | 63807 | 3602 | INIT=1 **DONE=1** EOS=1 GWE=1 CRC_ERR=0 | ~0.041s | 0 |
| bare | 63279 | 3574 | INIT=1 **DONE=1** EOS=1 GWE=1 CRC_ERR=0 | ~0.044s | 0 |

Example line (pin-wrap):

```
hw program backend=mpsse-sim part=HL10T-C32-1 frames=3602 bytes=63807 STAT INIT=1 DONE=1 EOS=1 GWE=1 GSR=0 GTS=0 CRC_ERR=0 (sim fabric via bitbang CFG_W; not board DONE)
```

`cargo test -p helion-hw --lib`: **21 passed** (native stub still NotImplemented; OFL parse refuses invented STAT).

## OFL / HAD / native honesty (cheap)

| Path | Claim |
|------|-------|
| `--cable mpsse-sim` | Real sim-fabric STAT after CFG_W DR — **not** board DONE |
| `--cable ofl` / USB | Still **TAP_readback=none**; never invent Helion STAT bits |
| `--cable native` | `NativeFtdiStub` **NotImplemented** → OFL fallback; detect-only enumerate |
| HAD `helion_hl10t` | Helion-local OFL `-b` alias; detect docs state TAP_readback=none + mpsse-sim STAT path |
| Box | No OFL binary / no FTDI — cannot claim USB/board DONE |

Detect note now points at `--cable sim|mpsse-sim` (not sim alone) and keeps native NotImplemented until real FTDI MPSSE.

## Honesty caveats

- **Sim fabric DONE only** for all three Ibex-scale bitstreams — IEEE 1149.1 bitbang via `Tap::tick`, in-process fabric.
- **Not** board DONE, **not** USB/MPSSE hardware, **not** openFPGALoader success.
- Pin-wrap / bare IOB=1 and imux_skip=0 are fabric-config quality from `fc95562`; this ship does not re-claim place work.
- Legal fence held: no UNISIM, no AMD IP/JTAG/XSim, no AXI-as-product, no Vivado trademarks, no Kintex clones.

## What remains (cheapest next)

1. Real FTDI MPSSE (`NativeFtdiStub` → driver) when hardware present — must read Helion STAT honestly or refuse DONE.
2. OFL bring-up with HAD + `HELION_OFL_BOARD=helion_hl10t` once probe + board alias exist; keep `TAP_readback=none` until native STAT works.
3. Do **not** claim board DONE without USB/OFL hardware evidence.

## Verdict

**PASS** — reduced + pin-wrap + bare Ibex `.hbits` build under ≤120s and program via `--cable mpsse-sim` with **STAT DONE=1**. Gold 9640 held. OFL/HAD honesty notes tightened (no invented TAP STAT; native NotImplemented). No merge. No uncapped cargo.
