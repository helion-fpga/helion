# Helion IP packages (`.helion`)

Each catalog core ships a **format 1** `.helion` text manifest next to its HDL:

```
format 1
vlnv community:helion:h_gpio:1.0
bus Helion-MM
top h_gpio
file h_gpio.v
```

Bus must be **Helion-MM** or **Helion-ST** (never AXI as a Helion product).

## Catalog

| Core | Bus | Notes |
|------|-----|-------|
| `h_gpio` | Helion-MM | Registered data + direction MM GPIO |
| `h_uart` | Helion-MM | Registered TX shifter + busy/div |
| `h_rv32_hb1` | Helion-MM | PicoRV32 wrap (not Zynq / not AXI) |
| `h_sync_fifo` | Helion-ST | Depth-8 / width-8 sync FIFO (wr/rd ptrs) |
| `h_timer` | Helion-MM | Loadable down-counter + enable/zero |
| `h_pwm` | Helion-MM | Period/duty regs, counter, output compare |
| `h_spi_mm` | Helion-MM | SPI master bit-engine (clk-div/shift/busy) |
| `h_debounce` | Helion-ST | N-stage shift-reg debounce, stable out |
| `h_clkdiv` | Helion-ST | Programmable clock divider (loadable div/en/out) |
| `h_edge_det` | Helion-ST | Rising/falling edge detect, sticky status + clear |
| `h_watchdog` | Helion-MM | Loadable countdown watchdog, enable/timeout/pet |
| `h_crc32` | Helion-ST | Byte-serial CRC32 (init/enable/data_valid) |
| `h_lfsr` | Helion-ST | Programmable-tap LFSR PRBS (enable/load/seed) |
| `h_gray_cnt` | Helion-ST | Binary↔gray counter with enable/load |
| `h_scratch_mm` | Helion-MM | 4×32b MM-mapped scratch regfile (wr/rd strobes) |
| `h_popcount` | Helion-ST | Registered 32b population-count |
| `h_saturate` | Helion-ST | Saturating add/sub + sticky signed OV flags |
| `h_shift_reg` | Helion-ST | Loadable 32b shift reg (dir/serial in/out) |
| `h_compare_mm` | Helion-MM | Threshold/compare regs + sticky match/irq |
| `h_clz` | Helion-ST | Registered 32b count-leading-zeros |
| `h_byte_rev` | Helion-ST | Registered endian byte-reverse (32b↔bytes) |

## Use

```bash
helion ip list
helion ip show ip/h_gpio/h_gpio.helion
helion ip show ip/h_timer/h_timer.helion
helion ip show ip/h_pwm/h_pwm.helion
helion ip show ip/h_debounce/h_debounce.helion
helion ip show ip/h_clkdiv/h_clkdiv.helion
helion ip show ip/h_edge_det/h_edge_det.helion
helion ip show ip/h_watchdog/h_watchdog.helion
helion ip show ip/h_crc32/h_crc32.helion
helion ip show ip/h_lfsr/h_lfsr.helion
helion ip show ip/h_gray_cnt/h_gray_cnt.helion
helion ip show ip/h_scratch_mm/h_scratch_mm.helion
helion ip show ip/h_popcount/h_popcount.helion
helion ip show ip/h_saturate/h_saturate.helion
helion ip show ip/h_shift_reg/h_shift_reg.helion
helion ip show ip/h_compare_mm/h_compare_mm.helion
helion ip show ip/h_clz/h_clz.helion
helion ip show ip/h_byte_rev/h_byte_rev.helion
helion project examples/ip_ingest/counter_ip.prj
helion project examples/ip_ingest/h_timer_ip.prj
helion project examples/ip_ingest/h_pwm_ip.prj
helion project examples/ip_ingest/h_debounce_ip.prj
helion project examples/ip_ingest/h_clkdiv_ip.prj
helion project examples/ip_ingest/h_edge_det_ip.prj
helion project examples/ip_ingest/h_watchdog_ip.prj
helion project examples/ip_ingest/h_crc32_ip.prj
helion project examples/ip_ingest/h_lfsr_ip.prj
helion project examples/ip_ingest/h_gray_cnt_ip.prj
helion project examples/ip_ingest/h_scratch_mm_ip.prj
helion project examples/ip_ingest/h_popcount_ip.prj
helion project examples/ip_ingest/h_saturate_ip.prj
helion project examples/ip_ingest/h_shift_reg_ip.prj
helion project examples/ip_ingest/h_compare_mm_ip.prj
helion project examples/ip_ingest/h_clz_ip.prj
helion project examples/ip_ingest/h_byte_rev_ip.prj
# in a .prj:
#   read_ip ip/h_crc32/h_crc32.helion
#   read_ip ip/h_gray_cnt/h_gray_cnt.helion
#   read_ip ip/h_scratch_mm/h_scratch_mm.helion
#   read_ip ip/h_popcount/h_popcount.helion
#   read_ip ip/h_saturate/h_saturate.helion
#   read_ip ip/h_shift_reg/h_shift_reg.helion
#   read_ip ip/h_compare_mm/h_compare_mm.helion
#   read_ip ip/h_clz/h_clz.helion
#   read_ip ip/h_byte_rev/h_byte_rev.helion
```

`read_ip` expands package `file` / `xdc` entries into the project source and constraint lists before synth.
