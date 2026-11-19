# CLK audit: what clocking encoding exists in public sources

Date: 2026-11-19. Worker: clocking (branch `fm/ultra-bitgen-clk`).
Sources checked, all on disk, no Vivado involved:

- prjuray-db (Project U-Ray, CC0 1.0), pinned commit
  `affbc5e555ebae16475f32e8fb2d6565d4204f3f`, `zynqusp/`:
  the vendored `crates/helion-ultrascale/data/segbits_cmt_right.db`
  (104 lines) plus the fifteen `segbits_rclk_*.db` files vendored by
  this worker (5,360 lines). The reference clone used for the check is
  `~/workspace/ultra-bitgen-crew/harness/reference/prjuray-db/`
  (`git rev-parse HEAD` returns the pinned commit).
- BitFiltrator `ULTRASCALE_PLUS.json` (MIT), vendored copy in the same
  `data/` directory. Top-level keys are exactly BRAM, CLEL_L, CLEL_R,
  CLEM, CLEM_R. Zero occurrences of the strings BUFG, CMT, CLK_H,
  MMCM, PLL, HDISTR, HROUTE anywhere in the file.
- Site-type JSONs in `data/`: BUFGCE, BUFGCTRL, MMCM, PLL (vendored
  earlier) plus BUFCE_LEAF, BUFCE_ROW, BUFCE_ROW_FSR, BUFGCE_DIV,
  BUFGCE_HDIO (vendored by this worker). These document site pins and
  site pips only, never bits.

No em dashes anywhere.

## What IS encodable (implemented in `src/clk.rs`)

### 1. CMT HROUTE/HDISTR routing (`CMT_RIGHT`, 104 lines)

`segbits_cmt_right.db`, all lines accounted for:

- 8 lines: `PIP.CLK_HROUTE_1_{5,6,8,9,14,17,20,21}.VCC_WIRE`, each a
  single must-be-zero bit (e.g. `PIP.CLK_HROUTE_1_5.VCC_WIRE !06_145`).
- 48 lines: `WIRE.CLK_HDISTR_0_0..23.USED.V0` / `.V1` pairs. V0 is the
  must-be-zero bit, V1 is the set bit at the same position
  (e.g. `CLK_HDISTR_0_0`: V0 `!07_33`, V1 `07_33`).
- 48 lines: `WIRE.CLK_HROUTE_1_0..23.USED.V0` / `.V1` pairs, same
  shape (e.g. `CLK_HROUTE_1_0`: V0 `!07_149`, V1 `07_149`).

Total 8 + 48 + 48 = 104. No BUFGCE, BUFGCTRL, MMCM, or PLL feature
appears in this file.

### 2. RCLK clock-distribution PIPs and USED bits (5,360 lines)

Every `segbits_rclk_*.db` file is PIP/WIRE lines except the two site
config groups in section 3 below. Per-file counts (data lines, from
`PROVENANCE.md`):

| File | Lines | Contents |
|---|---|---|
| segbits_rclk_ams_cfgio.db | 144 | 48 PIP + 96 WIRE, CLK_HROUTE_R* |
| segbits_rclk_bram_intf_l.db | 160 | 112 PIP + 48 WIRE |
| segbits_rclk_bram_intf_td_l.db | 160 | 112 PIP + 48 WIRE |
| segbits_rclk_bram_intf_td_r.db | 160 | 112 PIP + 48 WIRE |
| segbits_rclk_clel_l_l.db | 40 | 28 PIP + 12 WIRE |
| segbits_rclk_clem_l.db | 40 | 28 PIP + 12 WIRE |
| segbits_rclk_clem_r.db | 40 | 28 PIP + 12 WIRE |
| segbits_rclk_dsp_intf_clkbuf_l.db | 288 | 96 PIP + 192 WIRE, HDISTR FWD/REV pairs |
| segbits_rclk_dsp_intf_l.db | 80 | 56 PIP + 24 WIRE |
| segbits_rclk_dsp_intf_r.db | 80 | 56 PIP + 24 WIRE |
| segbits_rclk_hdio.db | 512 | 360 PIP + 144 WIRE + 8 BUFGCE_HDIO IINV (section 3) |
| segbits_rclk_int_l.db | 1576 | 1160 PIP + 416 BUFCE_LEAF config (section 3) |
| segbits_rclk_int_r.db | 1576 | 1160 PIP + 416 BUFCE_LEAF config (section 3) |
| segbits_rclk_intf_left_term_alto.db | 456 | 408 PIP + 48 WIRE, Zynq-PS-only BUFG_PS |
| segbits_rclk_xiphy_outer_right.db | 48 | 48 WIRE |

Feature spellings are sink-first, as in the INT PIP audit:
`PIP.<sink>.<source>`, e.g.
`RCLK_INT_L.PIP.CLK_LEAF_SITES_0_CLK_IN.CLK_HDISTR_FT0_0`
(11 tokens, one set bit plus ten must-be-zero bits) and the
bidirectional form `PIP.CLK_HDISTR_R0.CLK_HDISTR_L0.FWD` /
`.REV` (one set bit, one must-be-zero bit each).

### 3. Site config that DOES exist: BUFCE_LEAF and BUFGCE_HDIO IINV

These are the only clock site-config encodings in any public source.

**BUFCE_LEAF** (`RCLK_INT_L` / `RCLK_INT_R`, 32 sites each,
`BUFCE_LEAF_X0Y0` through `BUFCE_LEAF_X7Y3`, 13 features per site,
416 lines per file). Hand-checked site `BUFCE_LEAF_X0Y0` in
`segbits_rclk_int_l.db`:

```
BUFCE_LEAF.CE_TYPE.ASYNC 01_12
BUFCE_LEAF.CE_TYPE.SYNC !01_12
BUFCE_LEAF.CEINV.V0 !02_12
BUFCE_LEAF.CEINV.V1 02_12
BUFCE_LEAF.DELAY_TAP.V0 00_13 !00_14 !01_13 !02_13 !03_13
BUFCE_LEAF.DELAY_TAP.V1 !00_13 !00_14 01_13 !02_13 !03_13
BUFCE_LEAF.DELAY_TAP.V2 !00_13 !00_14 !01_13 02_13 !03_13
BUFCE_LEAF.DELAY_TAP.V4 !00_13 !00_14 !01_13 !02_13 03_13
BUFCE_LEAF.DELAY_TAP.V8 !00_13 00_14 !01_13 !02_13 !03_13
BUFCE_LEAF.IINV.V0 !00_12
BUFCE_LEAF.IINV.V1 00_12
BUFCE_LEAF.IN_USE.V0 !00_15
BUFCE_LEAF.IN_USE.V1 00_15
```

Notes, read off the DB, not assumed:

- CE_TYPE is a complement pair on one bit (ASYNC sets it, SYNC
  clears it). CEINV, IINV, IN_USE are V0/V1 complement pairs with
  V1 setting the bit, matching the FF INIT convention (V0 sets) only
  in the sense that each pair is complementary; the polarity per
  attribute is whatever the DB line says, and `src/clk.rs` resolves
  by exact feature lookup, never by a polarity rule.
- DELAY_TAP has exactly five values in the DB: V0, V1, V2, V4, V8.
  There is no V3, V5, V6, or V7 line. A tap value outside
  {0, 1, 2, 4, 8} is rejected by `src/clk.rs` as an invalid value,
  not rounded or guessed. The five lines for one site share five bit
  positions (frames 00..03, bits 13..14) in a one-hot pattern.
- The same 13 lines repeat for all 32 sites in each of the two files,
  at site-specific positions (verified by counting: 32 sites x 13
  features = 416 per file, and every site has exactly the same 13
  attribute spellings).

**BUFGCE_HDIO IINV** (`RCLK_HDIO`, 4 sites `BUFGCE_HDIO_X0Y0`,
`X0Y1`, `X1Y0`, `X1Y1`, 2 lines each, 8 lines total):

```
BUFGCE_HDIO_X0Y0.BUFGCE_HDIO.IINV.V0 !05_00
BUFGCE_HDIO_X0Y0.BUFGCE_HDIO.IINV.V1 05_00
```

Only IINV is encoded. There is no CEINV line and no CE_TYPE line for
BUFGCE_HDIO in the file, so only the input-inversion attribute of an
HDIO BUFGCE is encodable. The same file also carries 104 BUFGCE-named
PIP lines: `PIP.CLK_BUFGCE_HDIO_{0..3}_CLK_IN.<source>` (source is
`CLK_CMT_MUX_4TO1_{0..3}_CLK_OUT` or `CLK_INT_TOP`, a 2-way mux) and
`PIP.CLK_CMT_MUX_4TO1_{4..14}_CLK_OUT.CLK_BUFGCE_HDIO_{0..3}_CLK_OUT`
(a 4-way mux, 2 bits per selection).

## What is NOT encodable (fail loud, never guessed)

Checked by grepping every segbits file above plus the BitFiltrator
arch JSON for BUFGCE, BUFGCTRL, BUFGCE_DIV, MMCM, MMCME4, PLLE4, and
PLL. Results:

- **BUFGCE (CMT site)**: zero config lines. The site inventory
  (`site_type_BUFGCE.json`: pins CLK_IN, CLK_OUT, CE_PRE_OPTINV;
  site pips CEINV, IINV) proves the attributes exist; no DB line
  encodes them. A design instantiating a CMT BUFGCE fails loud with
  `ClkError::MissingEncoding`.
- **BUFGCTRL**: zero config lines. Site inventory
  (`site_type_BUFGCTRL.json`) lists CE0/CE1, I0/I1, IGNORE0/IGNORE1,
  SEL0/SEL1 inversion pips (note: the upstream JSON itself carries
  two garbled entries, `I1INV` sourced from `I0_PREINV` and
  `IGNORE1NV` sourced from `CKING1_PREINV`; those are upstream data
  errors, recorded here, not corrected by inventing entries).
  Fails loud the same way.
- **BUFGCE_DIV**: zero config lines. Site inventory exists
  (`site_type_BUFGCE_DIV.json`: CE, CLR, I inversion pips, RST pin).
  Fails loud.
- **BUFCE_ROW / BUFCE_ROW_FSR**: zero config lines. The sites appear
  only as PIP endpoints (e.g. `PIP.CLK_BUFCE_ROW_FSR_0_CLK_IN.*` in
  `segbits_rclk_clel_l_l.db`). Their CEINV/IINV site pips
  (`site_type_BUFCE_ROW.json`, `site_type_BUFCE_ROW_FSR.json`) have
  no bit encoding. Fails loud on any config request; their PIPs
  resolve normally.
- **MMCME4_ADV / PLLE4_ADV**: zero config lines in every public
  source. Site inventories exist (`site_type_MMCM.json`,
  `site_type_PLL.json`: pins only, no site pips for attributes).
  Any instantiation fails loud. SPEC.md out-of-scope rule applies:
  attribute fuzzing waits until a design needs it; the safe default
  is a design that instantiates no MMCM/PLL at all, and
  `src/clk.rs` treats any instantiated MMCM/PLL as blocked.
- **CMT_LEFT**: no segbits file exists at all in prjuray-db
  `zynqusp/` (only `segbits_cmt_right.db`). The xcku3p device file
  pairs CMT_L with BUFGCE, BUFGCE_DIV, BUFGCTRL, MMCM, and PLL, so
  the tile type that holds those sites on xcku3p has no public
  segbits coverage of any kind.
- **RCLK_INTF_LEFT_TERM_ALTO / BUFG_PS**: 384 BUFG_PS PIP lines
  exist, but they are Zynq-PS clocks (`PS_TO_PL_CLK*` sources). No
  PS exists on xcku3p/xcvu3p, so these lines must not be used for
  those parts. They are vendored for inventory completeness only.

## Why absolute emission is still blocked even for encodable features

`src/clk.rs` resolves features to tile-relative `(frame, bit)` refs,
exactly as the diff harness does (`harness/reference.py`,
`resolve_cmt_pip`, tile-relative). Absolute emission needs a base FAR
per CMT/RCLK tile, and there is none in any public source:

- `DeviceFile` (`src/bitgen_device.rs`) returns `None` for
  `tile_type` and `tile_base_far` on every HPIO/HDIO/CMT/DSP tile,
  by design: no column majors for those tiles exist in
  `xcku3p.json`, and no xcku3p/xcvu3p tilegrid exists publicly
  (prjuray-db ships tilegrids only for xczu3eg parts).
- The xczu3eg tilegrid does give RCLK tile base addresses (e.g.
  `RCLK_INT_L_X0Y29`: CLB_IO_CLK baseaddr `0x00000100`, 76 frames),
  but that is a different part; transferring its FARs to xcku3p
  would be invented data.
- Consequence: `emit_bitstream` on a FASM carrying any CMT/RCLK tile
  line fails loud today with `BitgenError::UnknownTile`. That is the
  correct behavior until the tilegrid lands; it is proven by a
  regression test in `tests/clk.rs`. The unblocker for absolute CLK
  emission is the xcku3p tilegrid (Vivado probe, Job 1 device-file
  method), not more segbits data.

## Golden design status

`crates/helion-ultrascale/tests/fixtures/clk_blinky.fasm` is a
BUFGCE-driven blinky: the blinky logic of the v1 golden plus the full
clock path (CMT BUFGCE, HROUTE/HDISTR USED, RCLK BUFCE_LEAF config,
INT GCLK PIP). Its CLK lines split exactly along the audit above:

- Resolvable tile-relative: CMT HROUTE/HDISTR USED, BUFCE_LEAF
  config, RCLK PIPs. Proven by `tests/clk.rs`.
- Blocked, with the exact missing data named: the CMT BUFGCE site
  config line (no encoding in any public source) and absolute FARs
  for every CMT/RCLK tile (no tilegrid).

The CLK block therefore does NOT go green on this golden. It is a
named blocker with two unblockers: (1) the Vivado clocking fuzz job
(`CLK_FUZZ_JOB.md`, Job 4) for BUFGCE/BUFGCTRL/MMCM/PLL site
config, and (2) the xcku3p tilegrid for CMT/RCLK base FARs. Nothing
in the golden is guessed, and no line is silently dropped.

## Counts summary

- Public clocking segbits lines total: 104 (CMT) + 5,360 (RCLK) =
  5,464. Vendored total in `data/`: 15,636 features across 24 files.
- Encodable site-config features: 832 BUFCE_LEAF lines (416 x 2 tile
  types) + 8 BUFGCE_HDIO IINV lines = 840.
- Missing site-config encodings: BUFGCE, BUFGCTRL, BUFGCE_DIV,
  BUFCE_ROW, BUFCE_ROW_FSR, MMCME4_ADV, PLLE4_ADV: 0 lines each.
