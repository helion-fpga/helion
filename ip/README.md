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

## Use

```bash
helion ip list
helion ip show ip/h_gpio/h_gpio.helion
helion ip show ip/h_timer/h_timer.helion
helion project examples/ip_ingest/counter_ip.prj
helion project examples/ip_ingest/h_timer_ip.prj
# in a .prj:
#   read_ip ip/h_timer/h_timer.helion
```

`read_ip` expands package `file` / `xdc` entries into the project source and constraint lists before synth.
