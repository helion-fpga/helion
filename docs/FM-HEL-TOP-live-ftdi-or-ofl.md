# FM-HEL-TOP — Live FTDI / OFL attempt + STAT TDO decode bar

**Date:** 2026-09-06 ~02:20 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `c907adb` / tip `ebbae5d` — Native FTDI MPSSE opcodes; no-device Io; refuse DONE until STAT TDO validated  
**Shipped:** SHA `dc98afa`  
**Gold:** `WNS_PS=9640` **held** (`helion report_timing examples/counter.sv --sdc examples/counter.sdc`)

## Goal

Try **live** FTDI / openFPGALoader on captain's Mac (`e6c67522-8627-418d-a593-df7fc43c59db`). If no probe reachable, ship the next software bar that still helps honest board DONE: STAT TDO capture/parse + mock harness.

## Live probe attempt (Mac)

| Check | Result |
|-------|--------|
| `ListMachines` | Mac listed `connected: true` (sakshams-MacBook-Pro-517.local) |
| Shell / Read via machineId | **Repeatedly failed:** `temporarily unreachable` (spawn error) — no `system_profiler`, no PATH OFL check, no Mac repo sync possible this turn |
| Box USB / FTDI | **0** FTDI (VID 0x0403); `openFPGALoader` not on PATH |
| OFL program | **Not run** — no probe + no OFL on box; never invent Helion TAP STAT |
| Native STAT on live device | **Not run** — no FTDI enumerated |

**Honesty:** No board DONE. Mac local-exec was unreachable despite ListMachines "connected"; treated as **no probe evidence this turn**.

## Software bar shipped (box → PR branch)

| Change | Where |
|--------|--------|
| `helion_shift_dr_u32_capture` — DR with INOUT TDO opcodes | `native_mpsse.rs` |
| `helion_read_stat` → capture path; `helion_read_stat_out_only` kept for bulk-OUT | same |
| `STAT_CAPTURE_TDO_LEN=5`, `pack_mock_stat_tdo`, `parse_stat_tdo_mpsse`, `stat_word_done` | same |
| `xfer_mpsse_inout` — bulk OUT + IN (EP 0x81) when `usb-native` | same |
| `read_stat` — parse live TDO → `Ok(Some(word))` or honest Io | same |
| `program_hbits` — Ok(()) **only** if live TDO parses with DONE bit5=1 | same |
| Mock roundtrip tests (STARTUP/RESET/random); short-buffer refuse | tests |
| Detect note: "STAT TDO decode mock-tested" | `native_mpsse_status_note` |

## Before → after

| Claim | Before (`c907adb`) | After |
|-------|---------------------|-------|
| STAT DR encode | out-only | **INOUT capture** opcodes |
| TDO → u32 parse | none (always refuse) | **`parse_stat_tdo_mpsse`** + mock roundtrip |
| Live DONE | refuse always after OUT | **Ok only if IN TDO parses DONE=1** |
| No device | Io | **unchanged** Io |
| Board / USB DONE | no | **still no** (no FTDI this host; Mac unreachable) |
| OFL Helion TAP STAT | TAP_readback=none | unchanged |

## Evidence

```
cargo test -p helion-hw --lib                 # 32 passed
cargo test -p helion-hw --lib --features usb-native  # 32 passed
helion report_timing examples/counter.sv --sdc examples/counter.sdc
# → WNS_PS=9640
./target/debug/helion-prog --detect --features usb-native build
# → native_ftdi probes 0; native_mpsse: … STAT TDO decode mock-tested
```

## Honesty caveats

- **No board DONE.** Mac Shell unreachable; box has no FTDI / no OFL.
- Mock TDO prove **decode**, not hardware. Live DONE still requires real FTDI + Helion TAP returning DONE=1 TDO.
- OFL still `TAP_readback=none` when used.
- Legal fence held; gold 9640; never uncapped Ibex; no merge / no force-push.

## What remains

1. When Mac local-exec works: `system_profiler SPUSBDataType`, OFL PATH, sync `fm-hel-corpus-soft-pass`, try OFL/`--cable native` on real probe.
2. Persistent FTDI handle + drain IN during combined CFG_W if needed.
3. Do **not** claim board DONE without USB/OFL hardware evidence.

## Verdict

**PASS (bar moved; live probe blocked)** — Mac unreachable → documented no probe. Shipped STAT TDO INOUT capture + parse + mock harness; native program Ok only on live DONE=1 TDO; gold 9640 held. No merge.
