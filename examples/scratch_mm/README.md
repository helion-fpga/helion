# Helion-MM scratch

Spine lab 4. Four 4-bit banks behind Helion-MM
(`mm_valid` / `mm_write` / `mm_addr` / `mm_wdata` / `mm_rdata`).
An internal host writes `A/5/3/C`; `led` is `mm_rdata[1]`. Not AXI.

## Expected (HL10T-C32-1, 10.000 ns user clock)

| | |
|---|---|
| cells | 97 |
| LUTFF | 48 |
| WNS_PS | 9540 |
| SOFT | 0 |

Numbers are from `helion project` (user XDC + LOC). Bare `helion qor` on the
`.sv` is a different packing (no PACKAGE_PIN) and is not this row.

## Headless

```
cargo run -p helion-cli -- project examples/scratch_mm/scratch_mm.prj
# create_clock=1 PACKAGE_PIN=2 cells=97 lutffs=48 WNS_PS=9540 SOFT=0
```

```
cargo run -p helion-cli -- project run examples/scratch_mm/scratch_mm.prj --cycles 16
# LED[16]=1010101010101010
```
