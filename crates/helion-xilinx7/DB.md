# Xilinx 7-series database sources (Worker A notes)

The raw Project X-Ray database is **not** committed to git. It lives at
`~/workspace/xilinx-dbsrc/` on the dev box (override with `HELION_XILINX_DB`).

## Provenance (all open data, CC0 / permissive)

- DB: `f4pga/prjxray-db` (CC0), `master` as of 2026-10-12.
  `xc7a35t` maps to the `xc7a50t` die fabric (`artix7/mapping/devices.yaml`:
  `"xc7a35t": {fabric: "xc7a50t"}`), so fabric files come from
  `artix7/xc7a50t/` and part/package files from `artix7/xc7a35tcsg324-1/`.
- Validation tools only (never runtime): `fasm2frames.py` and
  `tools/xc7frames2bit.cc` from `f4pga/prjxray`.
- Golden reference: `openXC7/demo-projects`, `blinky-digilent-arty/blinky.bit`
  (2.19 MB, matches 5408 frames x 101 words x 4 bytes) plus `blinky.v`,
  `blinky.xdc` (clk=E3, led=H5).

## openXC7/prjxray-db 0c3504f (checked 2026-11-19)

`openXC7/prjxray-db@0c3504f26f5cf77b6a7605c7bc83ebe1ac67b26f` (a fork of the
above) adds spartan7, kintex7 and virtex7, and works as the only database
root. Against f4pga master for artix7/zynq7: the xc7z020 tilegrid, tileconn,
part.json and package pins are identical; 74 of the 80 differing files only
add lines; the rest are zynq7's part list and a few ppips in HCLK_IOI3 and
GTP_COMMON tiles. With the same zedbit (b1c2c92) both databases give
byte-identical bitstreams for blinky, the PicoRV32 SoC, the MMCM design, the
Ibex SoC, the 240-RAM fill and the 192-DSP fill (ZedBoard), and for a counter
and PicoRV32 on all four boards (`tools/x7v/tests/boards.sh`); x7v verify
passes all of them decoding with 0c3504f. Neither database ships
`required_features.fasm` for xc7z020clg484-1; the loader takes the same die's
xc7z020clg400-1 file (G79). openXC7's spartan7 has none for any part.

## Layout under `~/workspace/xilinx-dbsrc/`

- `artix7/xc7a50t/tilegrid.json` (6.6 MB, 18055 tiles) and `tileconn.json`
  (11.9 MB, 748 stitch entries)
- `artix7/xc7a35tcsg324-1/part.json` (idcode `0x0362D093`, 5408 frames),
  `package_pins.csv` (212 pins)
- `artix7/tile_type_*.json` (128 files), `segbits_*.db` (56),
  `mask_*.db` (45), `ppips_*.db` (67)
- `tools/prjxray/`, `tools/openxc7-blinky/`

## Key facts baked into `helion-xilinx7`

- Tilegrid key: tile name (`INT_L_X0Y0`); entry has `grid_x/grid_y`,
  `type`, `sites` (name -> site type), `bits` (bus -> baseaddr/frames/offset/words).
- `tileconn.json` entries: `tile_types: [A, B]`, `grid_deltas: [dx, dy]`
  relative to the A tile; `wire_pairs: [[wire_in_A, wire_in_B], ...]`.
- Frame math: `frame = baseaddr + frame_offset`,
  `word = offset + bitpos / 32`, `bit = bitpos % 32`.
- The grid is the 50T die: 4075 CLB tiles = 8150 slices (the real 35T has
  5200; the DB simply reuses the 50T fabric, and the 35T bitstream frame
  count 5408 matches the golden blinky.bit size).
- E3 (clk) -> `IOB_X1Y76` on `RIOB33_X43Y75`, bank 35;
  H5 (led) -> `IOB_X1Y51` on `RIOB33_X43Y51`, bank 35.
- `segbits` bit tokens are `[!]frame_offset_bitpos`; this DB revision has no
  `<const0>`/`<candidates>` lines (parsers tolerate them anyway).
- Pip names: `TILE.SRC->DST` / `TILE.SRC->>DST`; `is_directional`/`is_pseudo`
  are loose `"1"`/`1` values.
