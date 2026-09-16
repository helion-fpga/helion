# hcore — Helion-original accumulator

Spine lab 5 (capstone). 4-bit acc, 2-bit PC, 4-instruction ROM.

ISA (2-bit op + 2-bit imm): `00` NOP, `01` LOAD, `10` ADD, `11` XOR.

Program: LOAD 1, ADD 2, XOR 1, NOP. `led = acc[0]`.

Original in-tree lab (Apache-2.0 OR MIT). Not PicoRV32, Ibex, or SERV.
Not AXI-as-product.

## Expected (HL10T-C32-1, 10.000 ns user clock)

| | |
|---|---|
| cells | 36 |
| LUTFF | 21 |
| WNS_PS | 9600 |
| SOFT | 0 |

## Headless

```
cargo run -p helion-cli -- project examples/hcore/hcore.prj
# create_clock=1 PACKAGE_PIN=2 cells=36 lutffs=21 WNS_PS=9600 SOFT=0
```

```
cargo run -p helion-cli -- project run examples/hcore/hcore.prj --cycles 16
# LED[16]=0011001100110011
```
