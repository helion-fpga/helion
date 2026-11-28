# Vivado 2017.3 synthesis-inference baseline (rtlsweep)

Part `xc7z020clg484-1`, `synth_design -mode out_of_context`, default options otherwise.
Each unit was run with `-flatten_hierarchy rebuilt` (the default), and again with `full` whenever the rebuilt netlist kept any hierarchy.
Ran on the USF `sc` partition with 4 threads per job, 2026-11-26.

## Files
| file | what |
|---|---|
| `vivado_synth.csv` | one row per (unit, flatten): status, synth_s, clk_port, wns_ns (100 MHz), blackboxes, hier_cells, LUT/FF/CARRY4/SRL/LUTRAM/RAMB18/RAMB36/DSP48E1/MUXF7/MUXF8, fsm_encoding, first ERROR line |
| `vivado_refcounts.csv` | (unit, flatten, ref_name, count) for every leaf or macro primitive (`IS_PRIMITIVE && PRIMITIVE_LEVEL != INTERNAL`) |
| `micro/` | the 84 `micro_*.v` modules and 2 `$readmemh` hex files: the regression corpus |
| `manifest_corpus/micro/extra/vhdl2008.txt` | unit, lang, top and file list exactly as synthesized |
| `skipped_files.csv` | files left out and why |
| `group_units.py`, `gen_micro.py`, `run_units.tcl`, `job.sh`, `collect.py` | the pipeline: group, rsync, sbatch, merge |

Column notes:
- LUT = LUT1..6. FF = FD*/LD*. SRL = SRL16E + SRLC32E.
- LUTRAM counts *cells* (RAM32M, RAM64M, RAM64X1D, ...), not LUTs.
- WNS is reg-to-reg only, because OOC has no I/O delays.
  - Blank WNS on a clocked unit means it has no reg-to-reg path.
  - Clock port: the first of clk/clock/clk_i/CLK/i_clk/clk_in that exists.

## Corpus
- Scanned 292 HDL files: 282 tracked `*.v/*.sv/*.vhd` in `helion-release`, plus 10 in `helion-zed-builds/vivado_jobs/rtl`.
- Skipped 72 (`skipped_files.csv`):
  - 34 byte-identical duplicates (`tools/x7v/tests/micro` mirrors `tools/svcheck/tests`; most of the zed-builds RTL mirrors `examples/`);
  - 31 Xilinx primitive models, UVM files and UltraScale-only fuzz tops;
  - 7 testbenches.
- 220 corpus units: every remaining file that declares a module or entity.
  - Dependencies are resolved transitively by module and package name, same directory first.
  - Top = the file's module that nothing in the unit instantiates; with several candidates, the one with the biggest subtree. `ysyx_ibex` was set by hand.
- Additional units:
  - 4 `__viv` units: the ibex wrappers rebuilt on `zed/ysyx_ibex_viv.sv`;
  - 1 `__vhdl2008` retry;
  - 84 `micro_*` units.
- In total, 309 units and 363 synth runs: 355 ok, 8 failed. The failures are 7 distinct units, 1 of them in both modes.
  - Corpus: 218 of 225 units synthesized.
  - Micro: 84 of 84.
- Four units synthesize to nothing after OOC optimization, because no observable logic is left:
  - `t01_iob_static`;
  - `ibex_reduced`;
  - `serv_debug`;
  - `svcheck hier_bitsel`.
- Largest units:
  - `zedboard_fill`: 45.6k LUT, 50k FF, 240 RAMB18, 192 DSP; 645 s.
  - `zedboard_dsp_fill`: 13.7k LUT, 2311 CARRY4, 192 DSP.
  - `sha256_all`: 2281 LUT, 104 CARRY4, 160 MUXF7; WNS -2.24 ns.
  - ibex: about 2.0k LUT, 136-155 FF, 21-44 CARRY4. Vivado trims most of the core in OOC, because only a few outputs are observable.
  - picorv32: 951 LUT, 678 FF, 83 CARRY4, 12 LUTRAM.
- Across the 192 rebuilt corpus units that were ok before the late additions:
  - CARRY4 in 69;
  - DSP48E1 in 14;
  - BRAM in 11;
  - LUTRAM in 11;
  - SRL in 5.
- Negative WNS at 100 MHz: `sha256_core`/`sha256_all` (-2.24 ns), `h_iir_biquad` (-1.36 ns), `dsp_wide_regs` (-0.24 ns).

## Inference rules learned (micro_*)

**Carry chains (CARRY4)**
- Add and sub use CARRY4 at every width tested, 8 bits included: w LUT + ceil(w/4) CARRY4. `add8` = 8 LUT + 2 CARRY4, `add64` = 64 + 16.
- Counters (`q <= q + 1`, with enable and sync reset):
  - 4 and 8 bits stay in LUTs only (cnt8 = 9 LUT, 0 CARRY4);
  - from 16 bits they use ceil(w/4) CARRY4 + a single LUT1, with CE and reset absorbed into FDRE.
  - The incrementer carry threshold lies in 9..16 bits (not probed further).
- Accumulators (`acc + d16`) use 16 LUT + w/4 CARRY4 at 16, 32 and 48 bits.
- Unsigned and signed `<` use CARRY4 from 8 bits: w LUT + w/8 CARRY4 (2 bits per LUT).
- `==`:
  - 8 bits = 3 LUT, no carry;
  - from 16 bits, a LUT6-plus-CARRY4 reduction: 16b = 6 LUT + 2 CARRY4, 32b = 11 + 3, 64b = 22 + 6.
- No carry chain in:
  - popcount32: 53 LUT, a LUT3/LUT5 tree;
  - prio32: 32 LUT;
  - barrel32: 160 LUT;
  - crc32 byte step: 42 LUT;
  - lfsr32: 1 LUT.

**DSP48E1**
- 8x8 never goes to a DSP: 90 LUT + 10 CARRY4 in the comb, 2-stage and 3-stage versions alike.
- 25x18 signed = 1 DSP, and every register is absorbed (AREG/BREG, MREG, PREG): 0 FF in comb, `_r` and `_p3`.
- 18x25 *unsigned* = 2 DSPs, because the unsigned 25-bit operand exceeds the signed 25-bit port. 17 FF stay in fabric in `_r` and `_p3`.
- 32x32 = 4 DSP + 47 LUT + 12 CARRY4. Its pipeline registers are not absorbed (`_r` 34 FF, `_p3` 98 FF).
- MAC (`acc <= rst ? 0 : acc + a*b`, 18x25 signed, 48-bit) = 1 DSP, 0 LUT, 0 FF. The accumulator, its reset and the pipeline registers all go into the DSP.

**SRL vs FF**
- Shift registers with CE and no reset:
  - depth 2 = 2 FF + 2 LUT3;
  - depth 3 = 3 FF;
  - depth 4 or more = SRL + 2 FF. Vivado keeps the first and last stages as flops: 4/16/32 = 1 SRL + 2 FF, 64 = 2 SRL + 2 FF.
- Any sync reset blocks SRL inference: all FFs (srl64_rst = 64 FDRE).
- A dynamic tap `sr[a]` on 32 stages = exactly 1 SRLC32E.

**LUTRAM vs BRAM**
- Sync write with async read always goes to LUTRAM, up to the largest size tested:
  - 16x8 and 32x8 = 2 RAM32M; 64x8 = 3 RAM64M;
  - 128x8 = 8 cells + 10 LUT; 256x8 = 16 cells + 12 LUT;
  - 512x8 = 32 cells + 24 LUT + 8 MUXF7.
- Sync read:
  - 64x8 stays in LUTRAM (3 RAM64M + 8 FF; the output register stays in fabric);
  - 256x8 and 1024x8 become 1 RAMB18. The threshold is between 64 and 256 words.
- BRAM shapes:
  - 512x36, 1024x18, 2048x9, 4096x4 and 16384x1 each use 1 RAMB18;
  - 1024x36 = 1 RAMB36, 2048x36 = 2 RAMB36;
  - true dual-port 1024x18 = 1 RAMB18;
  - byte-write 1024x32 (`mem[a][8*i +: 8]` loop) = 1 RAMB36 with no extra logic.
- ROMs:
  - 64x8 case ROM = 8 LUT6 + 8 FF;
  - `$readmemh` 256x16 and 1024x16 with a registered output = 1 RAMB18 each;
  - the log gives the rule that a ROM needs at least 5 address bits to go to BRAM (`Synth 8-5544`).

**Muxes**
- 4:1 = 1 LUT6; 8:1 = 2 LUT6 + 1 MUXF7; 16:1 = 4 LUT6 + 2 F7 + 1 F8.
- 32:1 = 9 LUT6 + 4 F7 and no MUXF8. The last level is a LUT6, not F8s.

**FSM** (`-fsm_extraction auto`)
- The 16-state FSM is extracted and encoded `sequential` (binary): 6 FF in total including the output, not one-hot.
- The 4-state FSM is not extracted at all (no `Synth 8-802`) and stays as 2 plain state FFs.

**Flatten**
- `-flatten_hierarchy full` changed results only for ibex: every ibex unit goes from about 2.0k to about 4.1k LUT with the same FF count.
- The other 49 `full` runs on hierarchical units match `rebuilt` exactly.

## Failures (7 units)
- `ysyx_ibex.sv`, plus the 4 wrappers that use it (`ibex_pin_wrap`, `ibex_obs_wrap`, `zedboard_ibex_soc`, `zedboard_ibex_mul`):
  - error: `[Synth 8-1019] vbits is not a valid constant function call` (ysyx_ibex.sv:6250);
  - Vivado 2017.3 rejects the source as it is. The `zed/ysyx_ibex_viv.sv` variant synthesizes, as do all 4 `__viv` wrapper units.
- `examples/blinky.vhd`: `[Synth 8-1031] std_logic is not declared`. The file has no `library ieee; use ieee.std_logic_1164.all;`, so it is not legal VHDL and Helion is being lenient.
- `micro_vhdl/variables_loop.vhd`: `[Synth 8-2757] only supported in VHDL 1076-2008`, because of the unary `or a`. It synthesizes with `read_vhdl -vhdl2008` (unit `__vhdl2008`).

## Caveats
- The unit grouping is regex-based. A wrong guessed top shows up as an unexpectedly tiny or empty netlist; check the four empty units above.
- Shard 3 of the corpus was partly re-run by a resume job by mistake. `collect.py` keeps the last row per (unit, flatten), so the results are not affected.
- After a resume, some flat units also got a `full` row. Those rows match `rebuilt`.
