# FM-HEL-TOP — ILA mark→arm fix + examples/ip read_ip

**Date:** 2026-09-06 ~08:19 America/New_York (EDT)  
**Branch:** `fm-hel-top` — PR https://github.com/helion-fpga/helion/pull/8 (**NO MERGE**)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**Tip:** `a5bf9c58653bce010ddce3b78dacfd4fb0ed0b8e` (`a5bf9c5`)  
**Remote:** push **only** `helion-fpga HEAD:fm-hel-top` (never `fm-hel-corpus-soft-pass`, never merge, never force-push master)

## Problem

1. **ILA:** `Session::mark_debug` / `insert_marked` already injects the probe LUTFF. `ila_arm` → `insert_arm_capture` then compiled baseline **with** the probe still present, so `insert_ila` was a no-op and the flow failed with `ILA insert was a no-op (bitstream unchanged)` on `examples/counter.sv` after mark → (re)implement → arm. Prior crumb test was weakened to avoid a false unwrap (5cb85d4).
2. **IP:** `.helion` + `read_ip` already existed (`examples/counter.helion`, `examples/ip_ingest/`). Needed a tiny **`examples/ip/`** surface + smoke that matches directory-form packages, with AXI legal fence held.

## What shipped

### ILA (real fix, small)

- `helion-debug`: `strip_ila(net)` removes prior probe cells/aux nets/endpoints.
- `insert_arm_capture` baselines a **stripped** design, then rebuilds the probe cleanly before the bitstream-diff / extra-LUTFF checks.
- Unit tests: `mark_debug_then_arm_inserts_probe_not_noop`, `strip_ila_removes_probe_cells` (structural counter gold `0000000111111110`).
- GUI: restore full `mark_debug cnt_3` → Place/Route/Bitstream → `ila_arm` in `ila_status_crumb_surfaces_mark_debug_and_capture`.

### IP (tiny examples/ip + smoke)

- `examples/ip/counter/package.helion` — directory form, Helion-MM, files via `../../counter.sv` + SDC.
- `examples/ip/counter.helion` — file form sibling.
- `examples/ip/read_ip_counter.prj` — `read_ip examples/ip/counter`.
- CLI smokes: `project_read_ip_examples_ip_dir_holds_gold`, `helion_ip_show_rejects_axi_fence`.

## Not touched

- Sim absolute split / `SPLITTER_GRAB_PX` / void Timing-Reports layouts.

## Gold

```
report_timing examples/counter.sv → WNS_PS=9640
examples/ip/read_ip_counter.prj   → WNS_PS=9640 LED[16]=0000000111111110
```

## Verify

```bash
cargo test -p helion-debug --lib
cargo test -p helion-cli --test project
cargo test -p helion-gui --lib ila_status_crumb_surfaces_mark_debug_and_capture
cargo run -p helion-cli -- report_timing examples/counter.sv
cargo run -p helion-cli -- project run examples/ip/read_ip_counter.prj --cycles 16
cargo run -p helion-cli -- ip show examples/ip/counter
```

## Remains (honest)

- ILA is still a soft probe (identity LUTFF + fabric `ble_q` readback), not a full UG908 on-chip capture RAM / JTAG upload path.
- CovertEDA-class encrypted IP vault / Generate Output Products GUI still open; text `.helion` + ingest only.
- No merge to master until Firstmate/captain.
