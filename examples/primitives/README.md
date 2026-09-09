# Helion primitive cells

Small open wrappers around Helion's own cells. Not a vendor architecture:
no sky130, UNISIM, Altera, or Xilinx primitives, and no cloned bitstream.

| Wrapper | Helion cell | What it is |
| --- | --- | --- |
| `HELION_LUT6` | `Lut6` | 6-input lookup. `INIT` is the 64-bit truth table, `I0` is the LSB of the address. |
| `HELION_FF` | `Hff` | Rising-edge flip-flop. Pins `CLK`, `D`, `Q`. |
| `HELION_IOB` | `IobOut` | Output pad. `I` is the fabric net, `PAD` is the top port. |

These files exist so a design can instantiate a documented Helion cell by name.
They are not a silicon library and they do not pretend to be one.
