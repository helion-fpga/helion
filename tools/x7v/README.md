# x7v: 7-series bitstream verification

Independent checks for Xilinx 7-series `.bit` files (ZedBoard, `xc7z020clg484-1`),
written against the Project X-Ray database only. It shares no code with Helion's
backend, so it can judge Helion's output without agreeing with itself.

```sh
cd tools/x7v
python3 -m x7v info      design.bit
python3 -m x7v decode    design.bit -o design.fasm      # every set bit -> feature
python3 -m x7v roundtrip design.bit                     # decode -> encode == input?
python3 -m x7v bram      design.bit                     # every configured block RAM half
python3 -m x7v dsp       design.bit                     # every configured DSP48E1
python3 -m x7v extract   design.bit -o netlist.v        # bitstream -> gate netlist
python3 -m x7v verify    design.bit --design blinky \
    --rtl blinky.vhd --top blinky --xdc blinky.xdc --golden vivado.bit
python3 -m x7v corpus    corpus.json [--bit-key golden]
```

Needs Python 3.10+, `nvc` (VHDL RTL), `iverilog` (netlist), and the X-Ray zynq7
database at `~/.cache/helion/prjxray-db/zynq7` (override with `X7V_DB`).

`flow`, `synthcheck` and `svcheck` run Helion's own tools from this checkout's
release build (`cargo build --release -p helion-xilinx7 --example zedbit`,
`cargo build --release -p helion-vhdl-zb --example synth2v --example vhdl2sv`);
`X7V_ZEDBIT`, `X7V_SYNTH2V` and `X7V_VHDL2SV` point elsewhere. `flow` builds
with the front end `zedbit` picks (`HELION_ZED_FRONT=auto|v2|zb`).

## Parts and boards

`--part PART` (before the command) checks a bitstream for any part the
Project X-Ray database describes; the family, fabric and package come from
the database's own `mapping/parts.yaml` and `devices.yaml` under
`X7V_DB_ROOT` (default `~/.cache/helion/prjxray-db`). `--board NAME` sets the
part from `boards/boards.json` and, for `flow`, takes pins from that board's
pools (clock on the oscillator, inputs on switches, buttons and input Pmods,
outputs on LEDs and output Pmods, then spare pins of its 3.3 V banks).
Without either, the part is the ZedBoard's (`xc7z020clg484-1`).

```sh
python3 -m x7v --board arty-a7-35 flow design.sv --top top
python3 -m x7v --part xc7a100tcsg324-1 verify design.bit --rtl design.sv --top top --xdc pins.xdc
tests/boards.sh [board ...]    # counter + PicoRV32 SoC per board, built and verified
```

Boards: `zedboard`, `arty-a7-35` (Arty A7-35T), `basys3`, `nexys-a7-100t`.
`verify` also checks that the bitstream's IDCODE is the part's: a device
refuses a bitstream written for another part.

What the Artix-7 checks rest on is weaker than the ZedBoard's: x7v decodes
and round-trips Vivado's Artix-7 harness bitstreams from the database
(mostly empty designs) with no unexplained bit, but there is no Vivado build
of real Artix-7 logic to compare against yet, and no Artix-7 board has been
tried.

`verify` options for larger designs:

* `--cycles N`, `--seed N`: length and seed of the random stimulus.
* `--reset-rate P`: chance per cycle that a reset input flips (default 0.01).
  A processor needs long runs between resets to get anywhere in its program.
* `--no-reset`: hold reset inputs at 0 for the whole run, with no start-up
  pulse, to prove that a design brings itself out of power-up (a power-on
  reset). Inputs count as reset when named `rst`, `reset`, `btnc` or `clr`.
* `X7V_RTL_SIM=verilator`: simulate SystemVerilog RTL with Verilator instead of
  Icarus, for sources Icarus cannot read (macros in `X7V_DEFINES`). Verilator is
  two-state, so a register the RTL never initialises reads 0, not X.
* `--axi-rw ADDR[=MASK],...` (Zynq PS7 designs): registers the AXI masters
  must read back as written, MASK being the bits they hold (default all 32).

## Zynq PS7 (`ps7.py`)

The PS7 has no PL configuration bits (its tiles' pips into the interconnect
are bitless always-on pseudo-pips), so x7v cuts the design at its pins.
Extraction makes each normal-class PS7 output (`ps7_ports.json`: FCLK,
FCLKRESETN, the AXI masters, EMIO, ...) a netlist input `PS7_<pin>` and each
input a netlist output; an input with no driver at all (an unused port's
clock, left unrouted) floats (`z`). Cosim gives the RTL a stub `PS7` with the
same ports (outputs set by the testbench, inputs sampled), so both sides get
one stimulus and every PS7 input (1,774 bits) and every pad output is compared
twice per cycle. An input the RTL leaves unconnected must read 0 or float,
never 1 (an unused interrupt on its interconnect default would be asserted).

The clock is the `create_clock ... [get_pins ps7/FCLKCLK[n]]` pin (else
FCLKCLK[0]). FCLKRESETN is one active-low reset, pulsed like any reset
input (`--no-reset` holds it high). Each M_AXI_GP port the RTL connects gets
an AXI3 master that first runs against the RTL: up to 4 reads and 4 writes
outstanding, VALID raised the cycle after the last handshake (back to back),
random AW/W/AR delays (W often before AW), random BREADY/RREADY, partial
strobes, 2-4 beat bursts, the first eight words of the window, the `--axi-rw`
registers and random words in it. Half the OKAY writes to an `--axi-rw`
register are read back next, and a read with no write in flight from its
address to its last beat must return what was written (under MASK). BID/RID
must match an outstanding ID and RLAST the last beat. What the masters drove
becomes the stimulus that both simulations replay. The `axi` check fails if
no write or no read completes, on any readback mismatch or protocol error,
and if an `--axi-rw` register is never read back. PS7 cosim runs on Icarus
only (it tells an unconnected input by `z`); the S_AXI ports, interrupts
and EMIO get the random walk.

`tests/ps7.py` builds examples/zedboard_ps7_axi.sv with zedbit (or takes
`--bit`) and checks: no bit in a PS tile, none unexplained; extraction gives
the BUFG FCLKCLK0, all 1,774 PS7 inputs as ports, 67 driven by the slave
and the rest 0 or floating; verify PASS with random resets and with
`--no-reset`. Negative controls, each FAIL: RDATA0 and RDATA1 swapped in the
bitstream (their interconnect muxes exchange sources; cosim only, on those
two bits), the IRQF2P0 tie pip removed (reads 1), and an RTL whose SCRATCH
ignores WSTRB[1] (the readback check).

## What `verify` checks

| check | meaning | failure on silicon |
|---|---|---|
| packets | command sequence equals Vivado's; both written CRCs are correct | wrong CRC: the device refuses to start |
| ecc | each frame's ECC (word 50, bits [12:0]) matches its contents | none (readback/SEU checks report corruption) |
| decode | every set bit is explained by an X-Ray feature | unknown configuration |
| clocks | clock spine and leaf enables present for every used clock path (`clockcheck.py`) | clock never reaches flops |
| extract | no undriven, multiply driven, or unmodelled logic | floating or shorted logic |
| cosim | extracted netlist equals the RTL, cycle by cycle, random stimulus | wrong behaviour |
| axi | (PS7 designs) the AXI masters' traffic completed, read back what it wrote, with no protocol error | none: proves the stimulus exercised the slave |
| io | I/O configuration equals the golden (Vivado) build, tile by tile | wrong standard, drive, pull, or a dead pin |

## Electrical model

* Nodes: tile wires joined by `tileconn.json`.
* Edges: active pip features, `always` pseudo-pips, and `default` pseudo-pips
  (taken only when nothing else drives the node: an unselected IMUX or BYP_ALT
  reads VCC).
* Site outputs are driven only by their site model (pseudo-pips into a site
  output are route-throughs the model already covers).
* `ZINV_*` features mean "not inverted"; their absence inverts.
* `CARRY4.xCY0` set selects the LUT's O5 as carry generate; clear selects the
  bypass pin (AX..DX).
* X-Ray numbers I/O sites from the top of the tile (`IOB_Y0` is the upper
  site); clock buffers (`BUFGCTRL`, `BUFHCE`) are named by in-tile position.
* `_SING` I/O tiles decode through their tilegrid alias (regular tile type,
  shifted by `start_offset` words, `_Y1` renamed `_Y0` when the offset is 0).
* Block RAM (`bram.py`): each BRAM tile is two RAMB18 halves (`RAMB18_Y0`,
  `RAMB18_Y1`), each modelled from its own bits (widths, SDP, write modes,
  DO_REG, RSTREG priority, INIT_A/B, SRVAL_A/B, INIT_xx/INITP_xx contents,
  `Z`-features stored inverted) by a behavioural Verilog RAM written from
  UG473 (`x7v_ramb18`, emitted into the extracted netlist). A RAMB36 is
  those two halves: the tile wires its RAMB36 pins to them bit-interleaved
  (`DOADO[2k]` lower half, `DOADO[2k+1]` upper) with a shared address, so
  the RAMB36 site itself drives nothing. FIFO, ECC, cascade, the RAMB36
  1-bit mode, per-byte write enables and WRITE_FIRST with unequal read and
  write widths are reported as extraction errors, not guessed. Address
  buses routed through the tile cascade wires (`BRAM_CASCINBOT_*` pips)
  need `CASCOUT_ARD/BWR_ACTIVE` in the driving tile below, as Vivado always
  sets it. Block RAM content frames carry no ECC (Vivado writes word 50 as 0).
* LUT RAM (`lutram.py`): a SLICEM LUT with `xLUT.RAM` is a 64-bit memory
  initialised from its INIT, read asynchronously on its own pins (O5 the
  lower half), written on the clock edge at the D LUT's pins `D1..D6`
  while the write enable (`CE` with `WEMUX.CE`, else `WE`) is 1. The data
  is the LUT's `xI` pin with `xLUT.DI1MUX.xI`, else the data of the LUT
  above (D always takes `DI`); `SMALL` LUTs write both 32-bit halves, the
  upper from the `xX` pin. `WA7USED`/`WA8USED` take `WA7`/`WA8` from
  `CX`/`BX`: C and A are written when `WA7` = 1, B and A when `WA8` = 1.
  Frame ECC: Vivado computes it with the INIT bits of RAM-mode LUTs taken
  as 0 (their contents change as the design runs).
* A LUT output reads only the inputs it depends on (O5 and O6 separately),
  so an X on an input one output ignores, such as a combinational loop the
  other output closes, does not reach it. Vivado's PicoRV32 has such a loop.
* DSP (`dsp.py`): each DSP tile holds `DSP48.DSP_0` and `DSP48.DSP_1`,
  decoded from their bits (AREG/BREG with the inverted `ZAREG_2_ACASCREG_1`
  cascade encoding; X-Ray lacks AREG_1/BREG_1, which Vivado writes as
  AREG_0's/BREG_0's frame-27 bit alone, the `Z<reg>` register-off bits, A/B_INPUT, USE_DPORT,
  USE_SIMD, AUTORESET_PATDET, SEL_MASK, PATTERN, MASK, `ZIS_*_INVERTED`)
  and modelled by a behavioural Verilog DSP48E1 written from UG479
  (`x7v_dsp48e1`: A/B/C/D/AD/M/P and control registers with CE and reset,
  INMODE, pre-adder, 25x18 multiply, OPMODE X/Y/Z, ALUMODE add/subtract and
  logic, CARRYINSEL, SIMD lanes, ACIN/BCIN/PCIN/CARRYCASCIN cascades over
  the tile's dedicated wires). The tie-off pips on DSP pins (`D`, `INMODE`,
  `CEAD`, ...) read their one bitless fabric source when neither is set.
  USE_MULT, USE_PATTERN_DETECT and SEL_PATTERN have no X-Ray bit, so
  AUTORESET, MACC extend (OPMODE Z = 100) and any read of PATTERNDETECT,
  PATTERNBDETECT, OVERFLOW, UNDERFLOW or MULTSIGNOUT are extraction errors.

## Validation (what has been proven, and on what)

Reference corpus: eight Vivado ZedBoard bitstreams (2023.2 and 2026.1) with
their VHDL and XDC: blinky, 4-bit adder (two architectures), BCD to binary,
shift register, up/down counter, pattern FSM, divider client/server.

* decode: 0 unexplained bits on all eight once ECC is accounted for.
* roundtrip: decode then encode reproduces all 9,996 frames bit for bit.
* ecc: matches Vivado on every frame of every bitstream.
* CRC: reproduces all 16 CRC values Vivado wrote.
* clocks: 0 violations and 0 unexplained enables on all clocked designs.
* extract + cosim: seven designs equal their RTL exactly (blinky over 2^25
  cycles, the rest over 3,000 random cycles). The pattern FSM differs, and the
  difference is in Vivado's bitstream itself: its state LUT never sees
  `prev_valid`, so the "valid fell -> S0" transition is missing.

Block RAM and DSP against Vivado 2017.3 (`xc7z020clg484-1`) builds of
examples/zedboard_bram_fill.sv (240 RAMB18, x18 READ_FIRST, address buses
through the tile cascade) and examples/zedboard_dsp_fill_prims.sv (192
DSP48E1, AREG = BREG = MREG = PREG = 1, P <= P + A*B): decode 0 unexplained
bits, ECC equal to Vivado's on every frame, extraction clean, and cosim 0
mismatches against the RTL (BRAM: 7,000 cycles, all three scans; DSP:
2,500 cycles with Xilinx's own unisim DSP48E1 as the RTL's model, which
shares nothing with `x7v_dsp48e1`). `tests/vivado_enc.py` checks the
encodings these builds taught x7v (and, given `X7V_VIVADO_REF`, the builds).

MMCM/PLL (`mmcm.py`, `python3 -m x7v cmt <bit> [--clkin-mhz F]`) against
Vivado 2017.3 builds of examples/zedboard_mmcm.sv and 51 generated sweep
designs (`tests/cmt_sweep_gen.py`, `tests/cmt_sweep_jobs.sh`): every CMT
decodes with 0 unexplained bits and no problem (counters, lock/filter words
against `cmt_tables.json`, POWER_REG, PHASE_MUX_F); zedboard_mmcm extracts
clean and co-simulates equal to its RTL (3,000 cycles; the RTL's MMCME2_BASE
and BUFG run on x7v's shims of the same clock model). `tests/cmt.py` checks
the decoder and, given `X7V_MMCM_REF` (the directory with Vivado's
zedboard_mmcm.bit), that build plus four negative controls (wrong divider,
MMCM held in reset, cut feedback loop, flipped lock-table bit), each of which
fails. Only internal feedback, integer dividers and tied-low control pins
are validated; the rest is refused.

LUT RAM against Vivado 2017.3 builds (`tests/lutram.py`, references in
`X7V_LUTRAM_REF`): every LUT RAM primitive side by side
(`tools/svcheck/tests_hard/lutram_prims.sv`, which also exercises F7/F8)
verifies against Xilinx's unisim models, 220,000 output samples, 0
mismatches, and fails with one RAM LUT's contents inverted; PicoRV32's
register file (12 RAM32M) verifies over 20,000 cycles; all 7 Vivado
LUT RAM builds decode with 0 unexplained bits and 0 ECC mismatches.

Not validated against Vivado (no reference design uses them yet): F7/F8
wide muxes outside LUT RAM (modelled, flagged as a warning), SRL,
IDDR/ODDR/SERDES, tristate I/O. The extractor reports these as errors rather
than guessing.

`tests/bram.py` composes its own: a zedbit-built base design plus one RAMB18
half placed and routed by the test over the X-Ray graph, re-encoded with
ECC and CRC. Four designs (x18 ROM with non-zero INIT_A, x18 READ_FIRST
writes, x9 WRITE_FIRST with DOA_REG, SDP 36) pass verify against plain
behavioural RTL; one flipped INIT bit and a cleared READ_WIDTH_A_18 each
fail in cosim. This proves decode, extraction and the model agree with
UG473 as x7v reads it, not that x7v reads the hardware right.
