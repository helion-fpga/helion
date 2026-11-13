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

## What has been checked

All of these equal their RTL on every output and every compared flop:

| design | size | cycles |
|---|---|---|
| `tools/svcheck/tests` (42 small designs) | up to a few hundred LUTs | 400 each |
| `ip/` library (70 blocks) | | 400 each |
| `examples/serv` (SERV, bit-serial RISC-V) | | 3,000 |
| `corpus-pass/sha256` | | 2,000 |
| `third_party/picorv32` (default and `ip/h_rv32_hb1` parameters) | 5.2k LUTs, 1.7k flops | 4,000 |
| `examples/ysyx_ibex.sv` (Ibex system, `HELION_DEFINES=VERILATOR`) | 11.5k LUTs, 2.2k flops | 5,000 |

Ibex needs one edit to its copy for Verilator to accept it as a reference
(`logic clk_sys = 1'b0` is also continuously assigned).

## Power-up values

`initial` blocks (assignments, loops, `$readmemh` / `$readmemb`) set the
power-up value of registers and memories. The bench runs the reference
simulation in the design's directory, so a relative memory-file path
resolves there as it does for Helion (next to the source file).
`init_readmemh.sv`, `init_readmemb_range.sv`, `init_regs_loop.sv` and
`init_readmem_param.sv` in `tests/`, and `ram_readmem.sv`,
`ram_init_deep.sv` and `rom_readmem.sv` (block RAM `INIT`, ROMs on block
RAM) in `hard/` check them; their files are in each `data/` directory.

## Block RAMs and multipliers

With `HELION_V2_HARD=1` the front end puts memories and products that a
Helion `Bram18` (1024 x 16, read-first, registered read) or `Mac27`
(16 x 16 -> 32) implements exactly on those blocks (see
`helion_sv::v2::set_hard_blocks`). The script models both cells as
`helion-ir` defines them, so

```sh
HELION_V2_HARD=1 python3 tools/svcheck/svcheck.py --corpus tools/svcheck/hard --regs --cycles 2000
```

checks the RAM and multiply designs in `tools/svcheck/hard` against their
RTL (all fifteen equal it): besides one-block RAMs and products, registered
products and products plus an addend (`mac.sv`, the P register and C
input), products wider than one block (`mul_wide.sv`, several blocks and
adders), memories wider than 16 bits (`ram_wide.sv`), deeper than 1024
words (`ram_deep.sv`), read at two addresses (`ram_2r.sv`) and all three
(`ram_2r_deep_wide.sv`), wider than 64 bits with an initial value
(`ram_w72_init.sv`), a registered product written through a wire
(`mac_wire.sv`) and registers and ports named like the nets made up for
the blocks (`ram_name_clash.sv`, `mac_wire.sv`). Without the variable those designs are built
from LUTs and flops, and every netlist is the same as before the option
existed.

## Reset

An asynchronous reset or set that is one plain signal with constant reset
values is built as a real asynchronous control, and the bench pulses resets
at random throughout. A block whose asynchronous control is anything else
(an expression, several controls, a reset value that depends on state) is
built as synchronous, with a warning; for such a design the bench holds
reset at start-up only, because a reset pulsed between clock edges would
reach those flops one edge later than in the RTL. `SVCHECK_PULSES=1` pulses
anyway.
