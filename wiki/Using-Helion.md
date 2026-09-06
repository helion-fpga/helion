# Using Helion

See the user guide: [use.html](https://helion-fpga.github.io/helion/use.html)

Gold: empty-XDC `examples/counter.sv` → `WNS_PS=9640`.

```
open dist/Helion.app
# or
cargo run -p helion-gui --bin helion-ide -- --headless examples/counter.sv
```

The GUI binary is `Helion` / `helion-ide`, not the CLI.

IP packages: `helion ip show ip/h_gpio/h_gpio.helion` (see `ip/README.md`).

Program honesty: `./scripts/ibex-prog-mpsse-sim-smoke.sh` for sim STAT; never claim board DONE without live FTDI/OFL evidence. Cap holds: `imux_skip=0`, `IOB=1`.
