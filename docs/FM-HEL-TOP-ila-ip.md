# FM-HEL-TOP — ILA mark→arm fix + deep fabric BRAM sample buffer

**Date:** 2026-09-09 ~23:10 America/New_York (EDT)  
**Branch:** `fm-hel-vivado-sim` (**NO MERGE** / soft-hold)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**Remote:** push **only** `origin fm-hel-vivado-sim` (never force-push, never rewrite history)

## Problem

1. Soft ILA (`insert_arm_capture`) programs fabric then host-polls `step_user` → `ble_out` / `ble_q`. Documented residual: not full on-chip capture RAM / JTAG upload (UG908).
2. Prior mark→arm no-op (probe already inserted) was fixed via `strip_ila` baseline.

## What shipped (P3 deepen)

### Soft path (unchanged gold)

- `insert_arm_capture` / GUI `ila_arm` still host `ble_out` readback.
- Counter soft gold `cnt_3` / `q3` window=16 → `0000000111111110`, WNS_PS=9640.

### Deep path (strictly beyond soft)

- Fabric runtime BRAM data plane: `Fabric::bram_write_word` / `bram_read_word`.
- `helion-debug`: `IlaArmConfig`, `IlaTriggerKind`, `IlaCaptureDeep`, `insert_capture_bram`, `insert_arm_capture_deep`.
  - Probe LUTFFs **plus** `ila_capture_ram` Bram18 in the bitstream (`Far::BRAM`).
  - Multi-probe window; samples packed into fabric BRAM each step.
  - Real trigger-before-fill (`pre_trigger`) for rising/falling; upload via BRAM readback.
  - Backend label: `fabric_bram_sample_buffer`.
- GUI/Tcl: `ila_pre_trigger N`, `ila_arm_deep [nets…] [window]` (soft `ila_arm` untouched).
- Unit tests: deep immediate matches soft q3 gold; multi-probe q0+q3; rising pre_trigger=4; GUI deep + soft gold hold.

## Gold

```
report_timing examples/counter.sv → WNS_PS=9640
soft q3/cnt_3 window=16 → 0000000111111110
```

## Verify

```bash
cargo test -p helion-debug --lib
cargo test -p helion-gui --lib ila_arm_deep_bram_pretrigger_multiprobe
cargo run -p helion-cli -- report_timing examples/counter.sv
# headless deepen proof:
printf '%s\n' 'open examples/counter.sv' 'implement' 'ila_window 16' 'ila_trigger rising' 'ila_pre_trigger 4' 'ila_arm_deep cnt_3,cnt_0' 'ila_dashboard' 'quit' \
  | helion-ide --stdin
```

## Remains (honest)

- Still **not** full UG908: no JTAG DR scan of capture RAM, no user-defined trigger FSM IP / match units, no compressed upload protocol.
- Deep path is fabric-sim BRAM sample buffer + bitstream-backed Bram18 cell — intermediate past soft `ble_out`, short of silicon ILA IP.
- Soft `ila_arm` remains the default GUI Arm button path.
- No merge to master until Firstmate/captain.
