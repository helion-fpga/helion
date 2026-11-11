# FM-HEL-TOP — ILA match-unit / trigger FSM IP + JTAG USR1 RLE upload

**Date:** 2026-09-09 ~23:50 America/New_York (EDT)  
**Branch:** `fm-hel-vivado-sim` (**NO MERGE** / soft-hold)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**Remote:** push **only** `origin fm-hel-vivado-sim` (never force-push, never rewrite history)

## Problem

1. Soft ILA (`insert_arm_capture`) programs fabric then host-polls `step_user` → `ble_out` / `ble_q`.
2. Prior deepen (ed29365 / 4992b4a / 4728722) added fabric BRAM + match-unit/FSM + raw IR_USR1 word-per-sample upload.
3. Documented residual after 4728722: **no compressed upload protocol** for the capture window.

## What shipped (P3 UG908 residual close — compressed upload)

### Soft path (unchanged gold)

- `insert_arm_capture` / GUI `ila_arm` still host `ble_out` readback.
- Counter soft gold `cnt_3` / `q3` window=16 → `0000000111111110`, WNS_PS=9640.

### Deep path (RLE over IR_USR1)

- Match unit + trigger FSM IP unchanged (`h_ila_trig` / `IlaTriggerFsm`).
- Capture BRAM fill unchanged.
- **Compressed upload**: TAP arms RLE on USR1 (TDI bit63|bit62 + `n_samples`), encodes the ring into run tokens, host scans header + tokens and expands.
  - Compact token: `run_len[63:48] | value[47:0]`
  - Escape (`run=0xFFFF`) + next DR for values with bits[63:48] set
  - Header magic `0xC0DE` carries `n_samples`, `n_tokens`, `n_dr`
- Backend label: **`jtag_usr1_match_fsm_rle`**
- Tests prove `upload_dr < upload_raw` on repetitive windows (and encode/expand round-trip).
- Soft `ila_arm` remains the default GUI Arm button path.

## Gold

```
report_timing examples/counter.sv → WNS_PS=9640
soft q3/cnt_3 window=16 → 0000000111111110
```

## Verify

```bash
cargo test -p helion-hw --lib usr1_rle
cargo test -p helion-debug --lib
cargo test -p helion-gui --lib ila_arm_deep_bram_pretrigger_multiprobe
cargo run -p helion-cli -- report_timing examples/counter.sv
printf "%s\n" "open examples/counter.sv" "run_implementation" "report_timing" "ila_window 16" "ila_trigger rising" "ila_pre_trigger 4" "ila_arm_deep cnt_3,cnt_0" "ila_dashboard" "ila_arm cnt_3" "quit" \
  | helion-ide --stdin
```

## Remains (honest)

- Still **not** full silicon UG908 (no board JTAG wire compression IP block, no Xilinx-compatible upload framing).
- Deep path is fabric match-unit LUT + trigger FSM IP + BRAM sample buffer + **IR_USR1 RLE upload** — past soft `ble_out`, past host-side edge trigger, past raw word-per-sample USR1.
- Soft `ila_arm` remains default.
- No merge to master until Firstmate/captain.
