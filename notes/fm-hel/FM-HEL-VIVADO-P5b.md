# FM-HEL-VIVADO-P5b — deepen MORE Helion-native catalog cores

**Date:** 2026-09-09 (~23:00 ET / America/New_York)  
**Branch:** `fm-hel-vivado-ip` (soft-hold, **NO MERGE**)  
**Base tip:** `50ad01f`

## What shipped

Two new Helion-native catalog cores with real sequential fabric (FFs+LUTs):

1. `h_pwm` Helion-MM — period/duty regs, free-running counter, output compare
2. `h_spi_mm` Helion-MM — SPI master bit-engine (clk-div, shift reg, busy/done); name does not imply AXI

Both ship format-1 `.helion` packages under `ip/<name>/` and are registered in `helion_ipxact::catalog()`.

## Proof

| Check | Result |
|---|---|
| `helion ip list` | 7 VLNVs — see `docs/FM-HEL-VIVADO-P5-catalog2.txt` |
| `helion project examples/ip_ingest/h_pwm_ip.prj` | `cells=229 luts=223 lutffs=223` — see `examples/ip_ingest/p5_cells2.txt` |
| `helion project examples/ip_ingest/h_spi_mm_ip.prj` | `cells=130 luts=124 lutffs=124` |
| `report_timing examples/counter.sv` | **WNS_PS=9640** — see `docs/FM-HEL-VIVADO-P5-gold-wns2.txt` |
| AXI as product | still rejected by `.helion` loader |

## Legal fence

No UNISIM / AMD IP / JTAG / XSim / AXI-as-Helion-product / Kintex clones. Buses Helion-MM or Helion-ST only.

## Verify

```bash
cd /Users/saksham/helion-ip
cargo test -p helion-ipxact --lib
cargo run -p helion-cli -- ip list
cargo run -p helion-cli -- project examples/ip_ingest/h_pwm_ip.prj
cargo run -p helion-cli -- report_timing examples/counter.sv
```
