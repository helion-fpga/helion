# ZB-12: Vivado equivalence and bitstream verification for the ZedBoard

- Track: ZedBoard, validation and bitgen.
- Branch: `fm/zedboard-verify` (from `fm/zedboard-fix-router` at 352c4d4).
- Status: bitgen layer is Vivado-identical on the reference corpus; the
  verification flow is validated; general-design P&R and synthesis are not
  done (defects listed below, with locations).

## Goal

Helion bitstreams for the ZedBoard (`xc7z020clg484-1`) that are equivalent to
Vivado's for any design, proven before they go on the board.

"Equivalent" is defined in two layers, because Vivado's placer and router are
closed and Vivado does not even reproduce itself across versions or
directives:

1. Encoding: for the same configuration (FASM), Helion's `.bit` must be
   byte-identical to Vivado's.
2. Function: for any design, the netlist the bitstream implements must equal
   the RTL, and every bit must be a documented X-Ray feature.

## Reference corpus

Eight Vivado-built ZedBoard designs with VHDL and XDC, kept outside the repo
in `~/helion-zedboard-corpus/` (coursework sources; `corpus.json` lists them):
blinky (25-bit counter, carry chain), 4-bit adder x2, BCD to binary, shift
register, up/down counter, pattern FSM, divider client/server.

## Findings

### Word 50 is the frame ECC

Every bit the earlier ZB-10 runs could not name (HCLK, HROW, CMT and
CFG_CENTER "word 50" bits) is the 13-bit per-frame ECC: a SECDED code over the
frame's other bits, stored in word 50 bits [12:0]. `frame_ecc` reproduces it on
all 9,996 frames of all eight Vivado bitstreams (2023.2 and 2026.1). The
`clkdist.rs` tables are ECC values of particular frames (e.g. `0x0643` is the
ECC of FAR `0x0040109b` in the blinky), so they are wrong for any other
design. With ECC computed, 0 bits are unexplained in any reference bitstream.

### The `_SING` I/O tiles are encodable

`tilegrid.json` gives every `*_SING` I/O tile a `bits.alias` record (regular
tile type, `start_offset` words, the site it maps to). Helion looked segbits up
by the tile's own type and rejected every line, so any pad on a `_SING` tile
(LED7 `U14`, several switches) was left unconfigured. Seven of the eight
reference designs use one.

### Vivado writes the configuration CRC

Helion reset the CRC (`CMD=RCRC`) where Vivado writes it. `icap_crc`
reproduces all 16 CRC values in the corpus, so Helion can write it too, and the
device then rejects a corrupted transfer instead of starting.

### Result: the bitgen layer is Vivado-identical

Decode each Vivado bitstream to FASM, assemble it with Helion
(`examples/fasm2bit.rs`): the configuration stream (everything after the
`.bit` header) is byte-identical to Vivado's for 8 of 8 designs. Before this
branch: ECC missing on every design, `_SING` lines rejected on 7 of 8.

### Helion's own builds of the corpus

`zedboard-handoff` CLI (`helion bitstream x.vhd --part xc7z020 --xdc ...`):

| design | result |
|---|---|
| adder x2, BCD | exit 0, `cells=0`: empty bitstream that does nothing |
| divider | exit 0, record type skipped, empty |
| shift register, counter, FSM | error, "net has 0 drivers" |
| blinky | 4 cells for a 25-bit counter |

`fm/zedboard-fix-router` (no xc7z020 CLI path; flow lives in
`tests/blinky_reg_e2e.rs`):

* carry blinky: one defect, now fixed (below). Passes every check; its
  extracted netlist equals the RTL over 2^25 cycles; 320 bits from Vivado.
* LUT (default) blinky: dead. All 26 flops read D = 0 (defect 2 below).

## Fixed on this branch

| change | file | proof |
|---|---|---|
| ECC computed for every frame in `fdri_payload` | `src/frames.rs` | golden round trip, 8/8 |
| `_SING` alias encoding (`TileBitLayout::alias`, `TileAlias`) | `src/frames.rs` | golden round trip, 8/8 |
| Configuration CRC written (`write_crc`, `icap_crc`) | `src/bitstream.rs` | 16/16 Vivado CRCs; `crc_tests` |
| PRECYINIT.C0 carry head keeps its AX bypass high | `src/emit.rs` | `emit_c0_carry_head_keeps_ax_bypass_high` fails before, passes after; blinky equals RTL for 2^25 cycles |
| FASM assembler for round-trip checks | `examples/fasm2bit.rs` | used by the round trip |
| Verification flow | `tools/x7v/` | see its README |

The carry head defect: the FF-slice "clock arrival" program grounds every
`BYP_ALT` of the serving INT tile through `GFAN0`/`GND_WIRE`. A C0 head computes
bit 0 from `DI0 = AX`, which must read 1 (`BYP_ALT1` for slice X0). Grounded,
bit 0 never carries and the counter is frozen. Removing exactly that one FASM
line made the extracted netlist match the RTL for 2^25 cycles; the emitter fix
reproduces that.

## Open defects (block "any design")

1. Clock pins are hardcoded: `emit.rs` writes `CLK0/CLK1.GCLK_B11` for every
   FF slice. That wire carries the clock only where the blinky's leaf buffer
   drives it. Route slice CLK through the real leaf/GCLK path instead, and
   emit the spine enables the `x7v` clock rules require (R1 to R4).
2. FF data routing: `route.rs` turns every FF D load into a sink on the slice
   pin `D` (route.rs, FF endpoints), which is the D LUT's output pin, while
   `emit.rs` selects `xFFMUX.AX..DX`. The bypass pin the FF reads is never
   routed. Combined with defect 3, every FF not fed by its own LUT reads 0.
3. Bypass muxes: the router refuses `BYP_ALT` for data (`is_byp_wire`, added
   in f5b9e12 to lower the blinky bit diff), and the emitter grounds all eight
   `BYP_ALT` of every FF tile. Replace with per-pin constants from the netlist
   (carry DI = 0 grounded, DI = 1 left at the VCC default) and let data use
   `BYP_ALT`.
4. FF model: `Hff` is D/Q/CLK plus optional CE and a sync reset to 0, INIT 0.
   No set, no async reset (`FFSYNC` is forced on every slice), no INIT 1, no
   control-set-aware packing.
5. Synthesis: on 7 of 8 real designs the VHDL path produces nothing usable, 4
   of them silently (exit 0). A front end that fails silently ships dark
   boards. Options: harden `helion-vhdl`/`helion-sv`, or import a Yosys netlist
   (GHDL plugin for VHDL). Gate either with `x7v` cosim on the corpus and the
   random suite.
6. No xc7z020 CLI on the current branch: only the stale `zedboard-handoff`
   fork has `helion bitstream --part xc7z020`. Port it (with ECC, alias, CRC).
7. Missing primitives: F7/F8, LUTRAM/SRL, block RAM, DSP48E1, MMCM/PLL,
   tristate I/O, multiple clocks.
8. `clkdist.rs`: superseded by `frame_ecc` (its bits are now overwritten by
   the real ECC). Remove it with its e2e assertions.

Gate for every change: `python3 -m x7v corpus ~/helion-zedboard-corpus/corpus.json --bit-key golden`
must stay green (proves the checker), and a Helion build must pass
`x7v verify` (cosim exact, I/O identical to Vivado) before it goes on the
board.

## Board test plan

Program with the command from the handoff (check the serial with
`openFPGALoader --scan-usb` first):

```sh
openFPGALoader -c digilent_hs2 --usb-serial-num <serial> --fpga-part xc7z020 <file.bit>
```

All files in `~/helion-zedboard-corpus/blinky/` unless noted.

| # | bitstream | expected | proves |
|---|---|---|---|
| 1 | `vivado.bit` | LD0 blinks, about 3 Hz | board and cable |
| 2 | `helion-blinky-carry.bit` | LD0 blinks, same rate as 1 | first verified Helion bitstream; Helion CRC and ECC accepted by silicon |
| 3 | `helion-router-carry.bit` (pre-fix) | LD0 stays off | x7v predicted this defect from the bitstream alone |
| 4 | `helion-blinky-lut.bit` | LD0 stays off | defect 2 predicted from the bitstream alone |
| 5 | `../updown_counter/helion-assembled.bit` (Vivado's FASM, assembled by Helion) | same as Vivado's `vivado.bit` there: holding BTNC shows SW0-3 on LD0-3; on release LD0-3 go to all on (SW4 up) or all off (SW4 down), since the counter saturates at 100 MHz | Helion bit writer (ECC, CRC, `_SING` pads) on silicon for a design with inputs |

`done=1` alone proves nothing (two earlier dark images reported it). Judge
every test by the LEDs.
