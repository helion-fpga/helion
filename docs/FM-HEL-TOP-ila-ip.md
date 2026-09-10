# FM-HEL-TOP — ILA mark→arm fix + JTAG USR1 capture-RAM upload

**Date:** 2026-09-09 ~23:30 America/New_York (EDT)  
**Branch:** `fm-hel-vivado-sim` (**NO MERGE** / soft-hold)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**Remote:** push **only** `origin fm-hel-vivado-sim` (never force-push, never rewrite history)

## Problem

1. Soft ILA (`insert_arm_capture`) programs fabric then host-polls `step_user` → `ble_out` / `ble_q`. Documented residual: not full on-chip capture RAM / JTAG upload (UG908).
2. Prior deepen (ed29365) added fabric BRAM sample buffer + trigger-before-fill, but upload was still host `bram_read_word` — not a JTAG DR scan of capture RAM.

## What shipped (P3 UG908 residual close)

### Soft path (unchanged gold)

- `insert_arm_capture` / GUI `ila_arm` still host `ble_out` readback.
- Counter soft gold `cnt_3` / `q3` window=16 → `0000000111111110`, WNS_PS=9640.

### Deep path (strictly beyond soft + prior BRAM deepen)

- Helion TAP **`IR_USR1`** (6-bit USER1-class): Capture-DR loads `bram_read_word(usr1_major, usr1_addr)`; Shift-DR scans **64-bit** LSB-first; Update-DR sets pointer (TDI bit63=1) or auto-increments / ring-wraps (bit63=0).
- `Tap::usr1_set_ptr` / `usr1_read_inc` / `usr1_upload_bram`; bitbang twin on `FtdiBitbangSim::shift_dr_u64` / `usr1_upload_bram`.
- `insert_arm_capture_deep` now arms on the **same** TAP fabric (step + BRAM fill), then uploads the window via **`Tap::usr1_upload_bram`** — not a host `bram_read_word` backdoor.
- Backend label: `jtag_usr1_bram_dr`.
- GUI/Tcl: `ila_arm_deep` / `ila_pre_trigger` unchanged at the command surface; dashboard reports new backend.
- Unit tests: TAP USR1 high-level + tick-accurate; mpsse bitbang upload; deep immediate still matches soft q3 gold; GUI deep + soft gold hold.

## Gold

```
report_timing examples/counter.sv → WNS_PS=9640
soft q3/cnt_3 window=16 → 0000000111111110
```

## Verify

```bash
cargo test -p helion-hw --lib usr1_
cargo test -p helion-debug --lib
cargo test -p helion-gui --lib ila_arm_deep_bram_pretrigger_multiprobe
cargo run -p helion-cli -- report_timing examples/counter.sv
# headless deepen proof:
printf '%s\n' 'open examples/counter.sv' 'run_implementation' 'report_timing' 'ila_window 16' 'ila_trigger rising' 'ila_pre_trigger 4' 'ila_arm_deep cnt_3,cnt_0' 'ila_dashboard' 'ila_arm cnt_3' 'quit' \
  | helion-ide --stdin
```

## Remains (honest)

- Still **not** full UG908: no user-defined trigger FSM IP / match units, no compressed upload protocol.
- Deep path is fabric-sim BRAM sample buffer + bitstream-backed Bram18 + **IR_USR1 JTAG DR upload** — intermediate past soft `ble_out` and past host BRAM readback, short of silicon ILA IP / match-unit FSM.
- Soft `ila_arm` remains the default GUI Arm button path.
- No merge to master until Firstmate/captain.
