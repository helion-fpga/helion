# Using Helion

See the user guide: [use.html](https://helion-fpga.github.io/helion/use.html)

Gold: empty-XDC `examples/counter.sv` → `WNS_PS=9640`.

```
open dist/Helion.app
# or
cargo run -p helion-gui --bin helion-ide -- --headless examples/counter.sv
```

The GUI binary is `Helion` / `helion-ide`, not the CLI.
