# UART (minimal 8N1 TX)

Spine lab 3. Auto-sends `8'hA5` LSB-first with start/stop. Helion-legal serial.
Not AXI UART.

## Expected (HL10T-C32-1, 10.000 ns user clock)

| | |
|---|---|
| cells | 10 |
| LUTFF | 5 |
| WNS_PS | 9640 |
| SOFT | 0 |

WNS matching the counter gold is coincidence (this is 5 LUTFF, not the 4-bit incrementer).

## Headless

```
cargo run -p helion-cli -- project examples/uart/uart.prj
# create_clock=1 PACKAGE_PIN=2 cells=10 lutffs=5 WNS_PS=9640 SOFT=0
```

Fabric TX over 16 cycles (sampled as the run LED):

```
cargo run -p helion-cli -- project run examples/uart/uart.prj --cycles 16
# LED[16]=0101001011010100
```

That is start + `8'hA5` LSB-first + stop, then the next start/data.
