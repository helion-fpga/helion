# FM-HEL-TOP — Native FTDI MPSSE opcode path + OFL/HAD honesty

**Date:** 2026-09-06 ~02:25 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `9ef8bdc` (+ stamp `cef8954`) — Ibex/pin-wrap/bare mpsse-sim STAT DONE=1; OFL honesty  
**Prior place:** SHA `fc95562` — imux_skip→0, IOB=1, gold 9640  
**Shipped:** SHA `c907adb`  
**Gold:** `WNS_PS=9640` **held** (`helion report_timing examples/counter.sv --sdc examples/counter.sdc`)

## Goal

Board-closer native FTDI **MPSSE opcode path** and/or OFL+HAD improvements — highest leverage honest ship **without** fake board DONE. Box has **no FTDI hardware**; still ship code+tests that prove the path exists and fails/fallback honestly.

## What shipped

| Change | Where |
|--------|--------|
| `MpsseOpcodeBuilder` — FTDI AN_108-class opcodes (TMS/TDI IR/DR, CFG_W packets, STAT) | `helion-hw` `native_mpsse.rs` |
| `NativeFtdiMpsse` (`HadUsbTransport`) — open FTDI when `usb-native`+device; encode CFG_W/STAT | same |
| Feature **off** → `NotImplemented` → OFL fallback (unchanged contract) | `try_native_usb_program` / `program_hbits_with_cable` |
| Feature **on**, **no device** → honest `Io` (no OFL soft-success, **never invent STAT**) | same |
| Device present → MPSSE open/bitmode + opcode xfer; still **refuses DONE** until STAT TDO validated | `program_hbits` / `read_stat` |
| `NativeFtdiStub` → delegates to `NativeFtdiMpsse` (compat) | `lib.rs` |
| HAD / detect notes: native MPSSE status, `helion_hl10t`, `TAP_readback=none` | `HAD_KNOWN_BOARDS`, `had_board_id_table_text`, detect |
| Tests: opcode encode w/o HW; feature gate; Io vs NotImplemented; OFL HAD notes | 29 lib tests |

## Before → after (honesty)

| Claim | Before (`9ef8bdc`) | After |
|-------|--------------------|-------|
| Native program | `NativeFtdiStub` always NotImplemented→OFL | **MPSSE opcode path** exists; feature-gated |
| `usb-native` + 0 FTDI | (enumerate only; stub NotImplemented) | **`Io` — no invented STAT** |
| `usb-native` off | NotImplemented→OFL | **unchanged** NotImplemented→OFL |
| MPSSE CFG_W/STAT **opcodes** | none (sim bitbang only) | **encoded** (`encode_cfg_w_and_stat` / `encode_read_stat`) |
| Board / USB DONE | no | **still no** (no FTDI on box; STAT TDO not validated) |
| OFL Helion TAP STAT | `TAP_readback=none` | unchanged + HAD notes tightened |
| mpsse-sim sim fabric DONE | yes | unchanged (not re-done) |

## Evidence

```
cargo test -p helion-hw --lib                 # 29 passed (feature off)
cargo test -p helion-hw --lib --features usb-native  # 29 passed
helion report_timing examples/counter.sv --sdc examples/counter.sdc
# → WNS_PS=9640
```

Detect (`helion-prog --detect` built with `--features usb-native`):

```
native_ftdi probes 0 feature=usb-native
native0 … MPSSE opcodes via NativeFtdiMpsse (… no device→Io; never invents STAT …)
native_mpsse: usb-native ON; 0 FTDI devices — open/program → Io (no invented STAT); …
had_board … ofl_board=helion_hl10t … OFL TAP_readback=none …
```

Native program without hardware:

```
./target/debug/helion-prog --cable native /tmp/counter.hbits
# → program: native MPSSE I/O (no invented STAT): no FTDI (VID 0x0403) device enumerated …
# exit ≠ 0; no DONE=1 claimed
```

## Honesty caveats

- **No board DONE.** No FTDI / no openFPGALoader on this box. Native path proves opcodes + open/error contract only.
- Even with a future FTDI attach, current `program_hbits` / `read_stat` **refuse DONE** until Helion TAP STAT TDO capture is validated (explicit `Io`, not a fake STARTUP_WORD).
- `mpsse-sim` remains the only path that reports sim-fabric STAT DONE (not board).
- OFL still reports `TAP_readback=none` / `STAT=(no readback)` on programmer-ok.
- Legal fence held: no UNISIM, no AMD IP/JTAG/XSim, no AXI-as-product, no Vivado trademarks, no Kintex clones.
- Gold 9640 held; no place/route redo; never uncapped Ibex.

## What remains

1. Attach real FTDI + Helion TAP: complete MPSSE TDO capture/parse for `IR_STAT` → then honest board STAT (or keep refusing).
2. OFL bring-up with HAD + `HELION_OFL_BOARD=helion_hl10t` when probe+alias exist; keep `TAP_readback=none` until native STAT works.
3. Do **not** claim board DONE without USB/OFL hardware evidence.

## Verdict

**PASS (bar moved, hardware still blocked)** — Native FTDI MPSSE **opcode path** shipped with tests; feature off → NotImplemented→OFL; feature on + no device → honest Io; never invents STAT; OFL/HAD notes updated; gold 9640 held. No merge.
