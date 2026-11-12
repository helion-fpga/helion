# FM-HEL-TOP — `.helion` IP/package format (gap7 leftover)

**Date:** 2026-09-06 (~02:45 ET / America/New_York)  
**Branch:** `fm-hel-corpus-soft-pass` (PR #7, **NO MERGE**)  
**SHA:** `a3da8312f655d2a8486f12d373af7aa4e129af0e` (`a3da831`)  
**Remote:** pushed to `helion-fpga/fm-hel-corpus-soft-pass`

## Problem (before)

Gap7 (`0293341`) shipped honest `.prj` multi-file + XDC + `helion project run`. Leftover called out in that report:

> Full CovertEDA-class IP catalog / `.helion` package format still open

Existing surface: `helion-ipxact` VLNV/XML + `ip/*.v` HDL + IDE catalog UI — **no on-disk package format** and **no project/synth ingest**.

## What shipped (smallest honest slice)

1. **Format 1 `.helion` text manifest** (`helion-ipxact`)
   - Directives: `format 1`, `vlnv` / vendor+library+name+version, `bus`, `top`, `file`, `xdc`
   - Directory form: `<dir>/package.helion`
   - Legal fence: **rejects AXI** bus (Helion-MM / Helion-ST only)
   - `load_helion` / `parse_helion` / `to_helion_manifest` / `HelionPackage::to_ip_core`

2. **Project ingest** (`helion-proj` + CLI)
   - `.prj` gains `read_ip <path.helion>`
   - `expand_ip_packages` resolves package files into `sources` + `constraint_files` (optional top from package if unset)
   - Wired into `helion project` / `helion project run` and `.prj` via `synth_any`

3. **CLI**
   - `helion ip list` — catalog VLNV (Helion-MM)
   - `helion ip show <file.helion>` — manifest + resolved paths
   - `helion ip pack <name> [-o out.helion]` — emit format-1 from catalog

4. **Packages + example**
   - `ip/h_gpio/h_gpio.helion`, `ip/h_uart/h_uart.helion`, `ip/h_rv32_hb1/h_rv32_hb1.helion`
   - `examples/counter.helion` + `examples/ip_ingest/counter_ip.prj` (gold path via `read_ip`)
   - `ip/README.md`, docs/use.html CLI bullet

## Before → after

| Check | Before | After |
|---|---|---|
| On-disk `.helion` package | none | format 1 manifest + HDL |
| `.prj` `read_ip` | n/a | expands into synth sources/XDC |
| `helion ip show ip/h_gpio/h_gpio.helion` | n/a | VLNV + Helion-MM + `h_gpio.v` |
| `helion project examples/ip_ingest/counter_ip.prj` | n/a | `ip=1 sources=1 xdc_files=1 WNS_PS=9640` |
| `helion project run …/counter_ip.prj` | n/a | gold LED `0000000111111110` |
| `report_timing examples/counter.sv` | WNS_PS=**9640** | **held** |
| AXI as Helion product | forbidden | loader rejects |

## Gold

- `WNS_PS=9640` (plain counter + `read_ip` counter package)
- LED `[16]=0000000111111110` via `project run` on `counter_ip.prj`
- Never uncapped Ibex; no UNISIM / AMD IP / AXI-as-product

## Verify

```bash
cargo test -p helion-ipxact --lib
cargo test -p helion-proj --lib
cargo test -p helion-cli --test project
cargo run -p helion-cli -- ip list
cargo run -p helion-cli -- ip show ip/h_gpio/h_gpio.helion
cargo run -p helion-cli -- project run examples/ip_ingest/counter_ip.prj --cycles 16
cargo run -p helion-cli -- report_timing examples/counter.sv
```

## Remains (honest)

- Not a full CovertEDA zip/encrypted IP vault or GUI “Generate Output Products” rewrite — text package + ingest only.
- Multi-file projects still SV-only for mixed VHDL/C lists (pre-existing gap7 remain).
- Catalog cores (`h_gpio`/`h_uart`/`h_rv32_hb1`) package for ingest; no new SoC BD auto-wire.
- No merge to master.
