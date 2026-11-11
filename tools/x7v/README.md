# x7v: 7-series bitstream verification

Independent checks for Xilinx 7-series `.bit` files (ZedBoard, `xc7z020clg484-1`),
written against the Project X-Ray database only. It shares no code with Helion's
backend, so it can judge Helion's output without agreeing with itself.

```sh
cd tools/x7v
python3 -m x7v info      design.bit
python3 -m x7v decode    design.bit -o design.fasm      # every set bit -> feature
python3 -m x7v roundtrip design.bit                     # decode -> encode == input?
python3 -m x7v extract   design.bit -o netlist.v        # bitstream -> gate netlist
python3 -m x7v verify    design.bit --design blinky \
    --rtl blinky.vhd --top blinky --xdc blinky.xdc --golden vivado.bit
python3 -m x7v corpus    corpus.json [--bit-key golden]
```

Needs Python 3.10+, `nvc` (VHDL RTL), `iverilog` (netlist), and the X-Ray zynq7
database at `~/.cache/helion/prjxray-db/zynq7` (override with `X7V_DB`).

## What `verify` checks

| check | meaning | failure on silicon |
|---|---|---|
| packets | command sequence equals Vivado's; both written CRCs are correct | wrong CRC: the device refuses to start |
| ecc | each frame's ECC (word 50, bits [12:0]) matches its contents | none (readback/SEU checks report corruption) |
| decode | every set bit is explained by an X-Ray feature | unknown configuration |
| clocks | clock spine and leaf enables present for every used clock path (`clockcheck.py`) | clock never reaches flops |
| extract | no undriven, multiply driven, or unmodelled logic | floating or shorted logic |
| cosim | extracted netlist equals the RTL, cycle by cycle, random stimulus | wrong behaviour |
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

Not validated against Vivado (no reference design uses them yet): F7/F8 wide
muxes (modelled, flagged as a warning), LUTRAM/SRL, block RAM, DSP, MMCM/PLL,
IDDR/ODDR/SERDES, tristate I/O. The extractor reports these as errors rather
than guessing.
