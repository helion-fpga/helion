# Example spine

Ordered in-tree labs. Empty-XDC `examples/counter.sv` gold is **WNS_PS=9640**
and is not moved. Soft CPU is last. Helion-MM / Helion-ST only — no AXI-as-product.

Headless (from the repo root):

```
./scripts/example-spine.sh
```

One lab:

```
./scripts/example-spine.sh counter
./scripts/example-spine.sh blinky
./scripts/example-spine.sh uart
./scripts/example-spine.sh scratch_mm
./scripts/example-spine.sh hcore
```

`cargo test -p helion-cli --test example_spine` gates every row below.

| # | Lab | Headless | cells | LUTFF | WNS_PS | SOFT |
|---|-----|----------|------:|------:|-------:|------|
| 1 | [counter](counter.sv) | `helion report_timing examples/counter.sv` | 9 | 4 | **9640** | 0 |
| 2 | [blinky](blinky.sv) + user clock + LOC | `helion project examples/blinky.prj` | 3 | 1 | 9700 | 0 |
| 3 | [uart](uart/README.md) | `helion project examples/uart/uart.prj` | 10 | 5 | 9640 | 0 |
| 4 | [scratch_mm](scratch_mm/README.md) | `helion project examples/scratch_mm/scratch_mm.prj` | 97 | 48 | 9540 | 0 |
| 5 | [hcore](hcore/README.md) | `helion project examples/hcore/hcore.prj` | 36 | 21 | 9600 | 0 |

`cells` is the mapped IR cell count (LUT6 + HFF + IOB). `LUTFF` is packed occupancy.
`SOFT=0` means no named incomplete cone (`WIDE_CONE`, `ASSIGN_NOT_LOWERED`, …).
UART WNS matching gold 9640 is coincidence (5 LUTFF, not the incrementer).

## 1. counter (gold, do not edit)

Empty-XDC path. LED = cnt[3].

```
cargo run -p helion-cli -- report_timing examples/counter.sv
# report_timing counter WNS_PS=9640 … SOFT=0
cargo run -p helion-cli -- run examples/counter.sv --cycles 16
# LED[16]=0000000111111110
```

Project form (`examples/counter.prj` + `examples/counter.sdc`) is the same 10 ns
clock and the same WNS; the gold *gate* is the empty-XDC file above.

## 2. blinky — pin LOC + user `create_clock`

RTL is `examples/blinky.sv` (QoR LUTFF=1 / WNS_PS=9700, unchanged).
User constraints live in `examples/blinky.sdc` (`create_clock` + `PACKAGE_PIN`),
wired by `examples/blinky.prj`. Not a default period.

```
cargo run -p helion-cli -- project examples/blinky.prj
# create_clock=1 PACKAGE_PIN=1 cells=3 lutffs=1 WNS_PS=9700 SOFT=0
```

## 3–5

Labs 3–5 live in their own directories (README + `.sv` / `.sdc` / `.prj`).
Each is Helion-legal SV: UART TX, Helion-MM scratch, then a Helion-original
accumulator core. No AXI interconnect, no vendor primitives.
