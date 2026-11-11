# FM-HEL-TOP — Persistent FTDI MPSSE session + OFL fixture dry/verify parser

**Date:** 2026-09-06 ~02:30 America/New_York  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `dc98afa` (+ stamp `55c6827`) — STAT TDO parse + mock; program Ok only if DONE bit5; Mac unreachable; 0 FTDI  
**Shipped:** SHA `ac12bdd`  
**Gold:** `WNS_PS=9640` **held** (`helion report_timing examples/counter.sv --sdc examples/counter.sdc`)

## Goal

Software board-closer without hardware: persistent FTDI handle/session for MPSSE (open once → CFG_W+STAT without reopen thrash) and/or OFL dry-run / verify-output parser tests with fixture logs (honest `TAP_readback=none`; never invent Helion STAT).

## Live probe (optional Mac retry)

| Check | Result |
|-------|--------|
| `ListMachines` | Mac `e6c67522-…` listed `connected: true` |
| Shell via machineId | **unreachable** (`temporarily unreachable`) — skipped (one attempt only) |
| Box FTDI / OFL | **0** FTDI; OFL not on PATH |

**Honesty:** No board DONE. No probe evidence this turn.

## Software bar shipped

### 1. Persistent FTDI MPSSE session

| Change | Where |
|--------|--------|
| `FtdiMpsseSession` holds `rusb::DeviceHandle` | `native_mpsse.rs` |
| `NativeFtdiMpsse.session` — open once; `usb_open_count` / `session_xfer_count` / `last_stat` | same |
| `open_probe` idempotent when session live (no reopen thrash) | same |
| `xfer_out_only` / `xfer_inout` use session handle (no re-enumerate) | same |
| `close_session()` drops handle | same |
| `program_hbits` requires ≥2 session xfers on same `usb_open_count` for Ok | same |
| `try_native_mpsse_program_stat` → validated STAT `u32` | same |
| `ProgramOutcome::NativeMpsse { bytes, stat_word }` — Ok only after live DONE=1 TDO | `lib.rs` |
| Fixed leftover refuse-Ok path that rejected validated native success | `program_hbits_with_cable` |
| Session lifecycle tests (no device → Io; close idempotent) | tests |

### 2. OFL dry-run / verify fixture parser

| Change | Where |
|--------|--------|
| Fixture logs under `crates/helion-hw/fixtures/ofl/` | sram Done, flash Verify OK/fail, CRC, dry-run note, generic Done |
| `ofl_fixture_logs_verify_and_tap_readback_none` | parses fixtures; SRAM/generic → `verify_ok=None` + `TAP_readback=none` |
| `ofl_dry_run_env_still_refuses_done_with_fixture_binary` | HELION_OFL_DRY_RUN=1 refuses DONE |

## Before → after

| Claim | Before (`dc98afa`) | After |
|-------|---------------------|-------|
| FTDI handle | open then **drop**; xfer **re-opens** each time | **persistent session**; CFG_W+STAT share handle |
| Native Ok in `program_hbits_with_cable` | refused even if STAT validated | **`NativeMpsse` outcome** with live STAT word |
| OFL verify parser fixtures | inline phrases only | **fixture log files** + dry-run refuse |
| Board / USB DONE | no | **still no** (0 FTDI; Mac unreachable) |
| OFL Helion TAP STAT | TAP_readback=none | unchanged (fixtures assert it) |

## Evidence

```
cargo test -p helion-hw --lib                 # 37 passed
cargo test -p helion-hw --lib --features usb-native  # 37 passed
helion report_timing examples/counter.sv --sdc examples/counter.sdc
# → WNS_PS=9640
helion-prog --detect --features usb-native
# → native_ftdi probes 0; native_mpsse: … persistent FTDI session (open-once) …
```

## Honesty caveats

- **No board DONE.** Mac local-exec unreachable; box has no FTDI / no OFL.
- Persistent session is proven structurally + no-device honesty; live open still needs real FTDI.
- OFL fixtures prove **parser** honesty (`TAP_readback=none`), not hardware program success.
- Legal fence held; gold 9640; never uncapped Ibex; no merge / no force-push.

## What remains

1. When Mac local-exec works: USB/OFL probe + sync branch + try `--cable native` / OFL on real cable.
2. Live exercise of persistent session (single open_count across CFG_W+STAT) on real FTDI + Helion TAP.
3. Do **not** claim board DONE without USB/OFL hardware evidence.

## Verdict

**PASS (bar moved; live probe blocked)** — Persistent FTDI MPSSE session + OFL fixture dry/verify parser tests; native Ok maps to `NativeMpsse` only after live DONE=1 TDO; gold 9640 held. No merge.
