# svcheck: does the synthesized netlist behave like the RTL?

```sh
cargo build --release -p helion-cli --example hnf
python3 tools/svcheck/svcheck.py design.sv [more.sv ...] [--top NAME] [--cycles N] [--regs]
python3 tools/svcheck/svcheck.py --corpus tools/svcheck/tests --regs
```

The design is synthesized with Helion's SystemVerilog front end, the
netlist is written out as gate-level Verilog, and both are simulated side
by side in Verilator under the same random inputs. Every output is compared
every clock cycle; with `--regs`, so is every flop against the source
register it holds, which pinpoints the first place a design goes wrong.

A net read but never driven, a net with two drivers, a cell the script does
not model, or an output bit without a driver is a failure, by name.

Needs `verilator` 5.x. Macros for the design come from `HELION_DEFINES`
(comma separated); `SYNTHESIS` is always defined.

## Block RAM and device primitives

With `HELION_V2_HARD=1` (what `zedbit` uses) the front end builds memories
with registered reads as `RAMB18E1` / `RAMB36E1` cells, memories block RAM
does not take (asynchronous reads, more than two ports, under 1 Kbit) with
one write port as LUT RAM (`RAM32M`, `RAM64M`, `RAM32X1S/D`, `RAM64X1S/D`,
`RAM128X1S/D`, `RAM256X1S`), and passes instances
of device primitives (`DSP48E1`, `MMCME2_ADV`, `BUFG`, ...) through as
cells. Both sides of the co-simulation then need Verilog models of those
primitives, found by module name (`NAME.v`) in `--models DIR` (repeatable,
searched first), `SVCHECK_MODELS` (colon separated), then `models/`.
`models/` holds Helion's own behavioural models of `RAMB18E1`, `RAMB36E1`
(true dual port, no ECC/cascade/output register), the nine LUT RAM
primitives, `BUFG`, `BUFGCE`, `CARRY4`, `SRL16E` and `SRLC32E`; Xilinx's
simulation library is not
bundled. `DSP48E1`, `MMCME2_*`, `PLLE2_ADV`, `IDDR` and `ODDR` need models
from `--models`.

```sh
HELION_V2_HARD=1 python3 tools/svcheck/svcheck.py --corpus tools/svcheck/tests_hard
```

`tests_hard` (run from the repository root: `rom_init` reads its hex file
by that path) covers single port read-first and write-first, simple and
true dual port with byte enables, contents from `initial` loops and
`$readmemh`, a memory split over rows of RAMB36, a ROM, hand-instantiated
primitives, and LUT RAM: a 32 x 8 register file with two reads
(`lutram_rf32x8`), 64 x 1 single port, 128 x 1 with two reads, 256 x 1,
64 x 8 with a registered read, a 128 x 16 asynchronous read
(`async_rd`) and every LUT RAM primitive by hand (`lutram_prims`).

## What has been checked

All of these equal their RTL on every output and every compared flop:

| design | size | cycles |
|---|---|---|
| `tools/svcheck/tests` (36 small designs) | up to a few hundred LUTs | 400 each |
| `ip/` library (70 blocks) | | 400 each |
| `examples/serv` (SERV, bit-serial RISC-V) | | 3,000 |
| `corpus-pass/sha256` | | 2,000 |
| `third_party/picorv32` (default and `ip/h_rv32_hb1` parameters) | 5.2k LUTs, 1.7k flops | 4,000 |
| `examples/ysyx_ibex.sv` (Ibex system, `HELION_DEFINES=VERILATOR`) | 11.5k LUTs, 2.2k flops | 5,000 |

Ibex needs one edit to its copy for Verilator to accept it as a reference
(`logic clk_sys = 1'b0` is also continuously assigned).

## Reset

An asynchronous reset or set that is one plain signal with constant reset
values is built as a real asynchronous control, and the bench pulses resets
at random throughout. A block whose asynchronous control is anything else
(an expression, several controls, a reset value that depends on state) is
built as synchronous, with a warning; for such a design the bench holds
reset at start-up only, because a reset pulsed between clock edges would
reach those flops one edge later than in the RTL. `SVCHECK_PULSES=1` pulses
anyway.
