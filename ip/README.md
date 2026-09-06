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

## Use

```bash
helion ip list
helion ip show ip/h_gpio/h_gpio.helion
helion project examples/ip_ingest/counter_ip.prj
# in a .prj:
#   read_ip ip/h_gpio/h_gpio.helion
```

`read_ip` expands package `file` / `xdc` entries into the project source and constraint lists before synth.
