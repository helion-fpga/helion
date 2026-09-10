# FM-HEL-TOP — ILA match-unit / trigger FSM IP + JTAG USR1 upload

**Date:** 2026-09-09 ~23:45 America/New_York (EDT)  
**Branch:** `fm-hel-vivado-sim` (**NO MERGE** / soft-hold)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**Remote:** push **only** `origin fm-hel-vivado-sim` (never force-push, never rewrite history)

## Problem

1. Soft ILA (`insert_arm_capture`) programs fabric then host-polls `step_user` → `ble_out` / `ble_q`.
2. Prior deepen (ed29365 / 4992b4a) added fabric BRAM + IR_USR1 JTAG DR upload, but **trigger was still host-side** on probe `ble_out` edges during arm.
3. Documented residual after 4992b4a: no user-defined trigger FSM IP / match units; no compressed upload.

## What shipped (P3 UG908 residual close — match FSM)

### Soft path (unchanged gold)

- `insert_arm_capture` / GUI `ila_arm` still host `ble_out` readback.
- Counter soft gold `cnt_3` / `q3` window=16 → `0000000111111110`, WNS_PS=9640.

### Deep path (past host ble_out trigger)

- **Match unit IP**: delay LUT+FF (`ila_match_prev`) + comb match LUT INIT
  (Immediate=`0xFFFF…`, Rising=`0x2222…` = I0&~I1, Falling=`0x4444…`) → `ila_match_hit`.
- **Trigger FSM IP**: BlackBox `ila_trig_fsm` / catalog `h_ila_trig`
  (`community:helion:h_ila_trig:1.0`, bus Helion-DBG) + fabric-resident
  `IlaTriggerFsm` (Idle→Armed→Fired→Done).
- Arm loop: `step_user` → `refresh_comb` → read **match-unit** `ble_out(ila_match_hit)` →
  `fab.ila_fsm.on_sample(match_hit)` — **not** host compare of consecutive probe samples.
- Capture BRAM fill + upload still via Helion **`IR_USR1`** 64-bit DR (`Tap::usr1_upload_bram`).
- Backend label: `jtag_usr1_match_fsm`.
- Soft `ila_arm` remains the default GUI Arm button path.

## Gold

```
report_timing examples/counter.sv → WNS_PS=9640
soft q3/cnt_3 window=16 → 0000000111111110
```

## Verify

```bash
cargo test -p helion-fabric --lib ila_trigger_fsm
cargo test -p helion-debug --lib
cargo test -p helion-ipxact --lib
cargo test -p helion-gui --lib ila_arm_deep_bram_pretrigger_multiprobe
cargo run -p helion-cli -- report_timing examples/counter.sv
printf '%s\n' 'open examples/counter.sv' 'run_implementation' 'report_timing' 'ila_window 16' 'ila_trigger rising' 'ila_pre_trigger 4' 'ila_arm_deep cnt_3,cnt_0' 'ila_dashboard' 'ila_arm cnt_3' 'quit' \
  | helion-ide --stdin
```

## Remains (honest)

- Still **not** full UG908: **no compressed upload protocol** for the capture window.
- Deep path is fabric match-unit LUT + trigger FSM IP + BRAM sample buffer + IR_USR1 upload —
  past soft `ble_out` and past host-side edge trigger; short of silicon compressed upload.
- Soft `ila_arm` remains default.
- No merge to master until Firstmate/captain.
