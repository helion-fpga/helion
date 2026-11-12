# FM-HEL-VIVADO-P5 — Helion IP / catalog depth

**Date:** 2026-09-09 (~22:55 ET / America/New_York)  
**Branch:** `fm-hel-vivado-ip` (soft-hold, **NO MERGE**)  
**Base:** `80117ff` (fm-hel-flow tip at P5 start)

## What shipped

1. Deepened `ip/h_gpio` and `ip/h_uart` — registered Helion-MM data/dir and TX shift/busy/div.
2. New catalog cores:
   - `h_sync_fifo` Helion-ST (depth 8 / width 8, wr/rd ptrs, full/empty, storage regs)
   - `h_timer` Helion-MM (loadable down-counter, enable, zero status)
3. Catalog + AXI reject tests updated in `helion-ipxact`.
4. `examples/ip_ingest/h_timer_ip.prj` instantiates `h_timer` via `read_ip` and synthesizes real fabric.

## Proof

| Check | Result |
|---|---|
| `helion ip list` | 5 VLNVs (uart/gpio/rv32/sync_fifo/timer) — see `docs/FM-HEL-VIVADO-P5-catalog.txt` |
| `helion project examples/ip_ingest/h_timer_ip.prj` | `cells=213 luts=209 lutffs=209` — see `examples/ip_ingest/p5_cells.txt` |
| `report_timing examples/counter.sv` | **WNS_PS=9640** — see `docs/FM-HEL-VIVADO-P5-gold-wns.txt` |
| AXI as product | still rejected by `.helion` loader |

## Legal fence

No UNISIM / AMD IP / JTAG / XSim / AXI-as-Helion-product / Kintex clones. Buses Helion-MM or Helion-ST only.

## Verify

```bash
cd /Users/saksham/helion-ip
cargo test -p helion-ipxact --lib
cargo run -p helion-cli -- ip list
cargo run -p helion-cli -- project examples/ip_ingest/h_timer_ip.prj
cargo run -p helion-cli -- report_timing examples/counter.sv
```
