# Reference data provenance

All reference data is CC0/public-domain or MIT. No RapidWright .dat,
no FPGA Interchange DBs, nothing under a Xilinx EULA. Nothing here was
downloaded from a license-gated source.

## prjuray-db (f4pga/prjuray-db, CC0 1.0)

- Location: `reference/prjuray-db/` (sparse clone, `zynqusp` only).
- Pinned commit: affbc5e555ebae16475f32e8fb2d6565d4204f3f
  (`git rev-parse HEAD` in that dir).
- Cloned 2026-11-07 with `git clone --depth 1 --filter=blob:none
  --sparse https://github.com/f4pga/prjuray-db.git`, then
  `git sparse-checkout set zynqusp`.
- Trimmed after cloning (documented here so it can be re-fetched):
  removed the second part dir `xczu3eg-sfvc784-1-e` (its tilegrid.json
  is byte-identical to `xczu3eg-sbva484-1-e/tilegrid.json`), both
  `tileconn.json` files (81M each, not needed by the harness),
  `timings/`, and `tile_types/` (18M, re-fetchable from the pinned
  commit). Kept: all `segbits_*.db`, `site_types/`, one part dir with
  `tilegrid.json` (15M, used to derive the INT column rule),
  `part.json`, `part.yaml`, `package_pins.csv`.
- Used: `segbits_int.db` (3778 PIPs), `segbits_clel_l.db`,
  `segbits_clel_r.db`, `segbits_clem.db`, `segbits_clem_r.db` (LUT/FF
  cross-checks), `segbits_bram.db`, `segbits_bram.block_ram.db`
  (BRAM cross-check), `segbits_hpio_right.db` (1560 lines),
  `segbits_cmt_right.db` (104 lines), `site_types/` (missing-data
  naming, e.g. DSP48E2 pins-only).
- Caveat: `zynqusp` is Zynq UltraScale+; the INT PIP and CLB encodings
  are tile-type level and are expected to transfer across UltraScale+
  parts of the same family (scout report). The xcku3p/xcvu3p tilegrids
  (per-tile base FARs) are NOT in prjuray-db: only xczu3eg parts ship
  tilegrids. That is why IO/CLK reference bits stay tile-relative and
  the INT column rule is provisional.

## BitFiltrator (EPFL VLSC, MIT License)

- Location: `reference/ULTRASCALE_PLUS.json`,
  `reference/xcku3p.json`, `reference/LICENSE`, `reference/README.md`.
- Source clone: `~/workspace/helion-ultra-bitstream/bitfiltrator-ref/`
  (github.com/epfl-vlsc/bitfiltrator), commit
  79b9bd66ecae31354936143180fa578dc5732baf.
- Used: `ULTRASCALE_PLUS.json` (`CLEL_L/R`, `CLEM/R` LutLoc/RegLoc,
  `BRAM` BramMemLoc/BramMemParityLoc) and `xcku3p.json` (SLR0
  rowMajors: clb_colMajors, clb_tileTypes, bram_content_colMajors,
  bram_content_parity_colMajors, IDCODE 0x04a63093).
- The MIT LICENSE file is vendored alongside the JSONs. The paper
  (fpl22-bitfiltrator-paper.pdf) and `src/bit_locator.py`,
  `src/bitstream_state_checker.py`, `src/u-ray_clb_comparator.py`
  were read (not copied) to pin down the resolution flow, the FF INIT
  inversion (XAPP1230), and the prjuray cross-check method. The
  harness reimplements that flow in `reference.py` and cross-checks
  its output against the prjuray DBs on every run.

## Cross-source agreement (verified, not assumed)

- LUT: prjuray `CLEL_L.ALUT.INIT[0] 11_15` equals BitFiltrator LutLoc
  pair [0] (minor 11, offset 15); all 64 bits, all 8 LUTs, all 4 tile
  types agree (checked every run, zero mismatches).
- FF: prjuray `AFF.INIT.V0` bit equals BitFiltrator RegLoc (minor,
  offset); V1 is its complement; all 16 FFs agree. Polarity is
  inverted (frame bit = 1 - INIT) per XAPP1230 via BitFiltrator's own
  state checker.
- BRAM: prjuray `BRAM.RAMB18E2_L.INIT_00[0..4] 00_00/00_12/00_24/
  00_36/00_60` equals BitFiltrator BramMemLoc Y0 frame_ofst
  [0,12,24,36,60] with minors [0,0,0,0,0].
- Frames: prjuray frame numbers are absolute minors within the
  tile's column (INT tile: 76 frames at base minor 0; CLEL tile: 16
  frames at base minor 0), verified against the xczu3eg tilegrid
  baseaddrs (INT_X11Y60 0x00042300, CLEL_R_X11Y60 0x00042400).
  INT column major = CLB column major - 1 in that grid; provisional
  for xcku3p/xcvu3p.
