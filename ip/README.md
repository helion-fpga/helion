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

## Use

```bash
helion ip list
helion ip show ip/h_gpio/h_gpio.helion
helion ip show ip/h_timer/h_timer.helion
helion ip show ip/h_pwm/h_pwm.helion
helion ip show ip/h_debounce/h_debounce.helion
helion ip show ip/h_clkdiv/h_clkdiv.helion
helion project examples/ip_ingest/counter_ip.prj
helion project examples/ip_ingest/h_timer_ip.prj
helion project examples/ip_ingest/h_pwm_ip.prj
helion project examples/ip_ingest/h_debounce_ip.prj
helion project examples/ip_ingest/h_clkdiv_ip.prj
# in a .prj:
#   read_ip ip/h_pwm/h_pwm.helion
```

`read_ip` expands package `file` / `xdc` entries into the project source and constraint lists before synth.
