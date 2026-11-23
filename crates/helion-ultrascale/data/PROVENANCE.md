# Provenance: vendored segbits data

Every file in this directory is vendored from a public source under a
permissive license. No RapidWright .dat files and no FPGA Interchange DBs
were ingested (both are REJECTED by the crew's legal line). No file here
has unverified provenance.

## Project U-Ray DB (prjuray-db)

- Source repo: https://github.com/f4pga/prjuray-db (Project U-Ray)
- Clone commit: `affbc5e555ebae16475f32e8fb2d6565d4204f3f`
  (2020-07-21, merge of PR #2, "next_zynqusp_db")
- License: CC0 1.0 Universal. Verified 2026-11-07: the clone's `COPYING`
  file is the full CC0 1.0 legal code (Creative Commons, statement of
  purpose plus waiver). Re-cloned fresh 2026-11-07 into
  `~/workspace/prjuray-db-ref` (the day-1 `/tmp` clone was wiped by a
  deployment restart); the loader never reads that clone or `/tmp`.
- Vendored: 2026-11-07, worker segbits-vendor, branch
  `fm/ultra-bitgen-segbits`.

### Segbits DBs (`.db` files)

Each `.db` file carries a 4-line `#` header naming source repo, commit,
and license. The data lines below the header are byte-identical to
upstream; the SHA-256 values below are of the upstream data (header
excluded), so they verify against the source repo. Line counts exclude
the 4 header lines.

| File | Upstream lines | SHA-256 of upstream data |
|---|---|---|
| segbits_int.db | 3778 | f53f8d60b18d4f0d5902decd09aa11b4c2f4f32affe4561cc018f2c2fac0143f |
| segbits_hpio_right.db | 1560 | e710286cf9f2abefef1dcae2414de65955bd5fe2dacf36eed72137e1128f9e84 |
| segbits_hdio_top_right.db | 812 | 447d033863858ec0d712625bbd8370f8b0243f8a96ea80d7979b89eba9cff28b |
| segbits_hdio_bot_right.db | 828 | 7dccae1fdba384c2ba39baf883ab61579857b5dc02e578bd65eb259d401654ba |
| segbits_cmt_right.db | 104 | 01d806d0f55ba62ed2af2deafa5c2160f00477bc76ff7df2a5c69cac40ca4459 |
| segbits_clel_l.db | 768 | c977e335ce757de5e0caa4d161de932dedf5b4d4c8c97f663680ee6506bc2501 |
| segbits_clel_r.db | 768 | f6564ea5b04f25dd77c22d41d45348af407243d84894689ecb4204df2ae52fa1 |
| segbits_clem.db | 829 | c1d4c6c156df25d6dad6f2b62610e5733933222d7d046e935db90ed57bea2bb2 |
| segbits_clem_r.db | 829 | b8b40c13498ffe89c9c79d27cb64200fa3e4db1e789ded8b0709dd4400468bc7 |
| segbits_rclk_ams_cfgio.db | 144 | 12f72f844e6f1df38f993a82879ce826ca15385709268c78de502c8391fa98f1 |
| segbits_rclk_bram_intf_l.db | 160 | 630a3b350c7f194e608ca1c583a4e5bc5904615ab33a7e0434ebd6f1dddcdbc9 |
| segbits_rclk_bram_intf_td_l.db | 160 | 6d10f8e041f257af0a6434c9ac9dae8dda604ea2436b01a5c45f24c0fbfccd39 |
| segbits_rclk_bram_intf_td_r.db | 160 | 8aeddf7e1db8339759645d383239bdf97598e0607ecc84bcf872c62724c51de0 |
| segbits_rclk_clel_l_l.db | 40 | 1d2e97cd2af627b08647b815fe7639accf638718016b14bd837490e23cb4a704 |
| segbits_rclk_clem_l.db | 40 | 0bd0c488d4dc8445ffb4687288d470f8671cb0fff7234eb3a84450e31278e109 |
| segbits_rclk_clem_r.db | 40 | 1ac18a6e6c56ee92f9220229680cdafd8a10f41b13265eca1a53aa5d76fcd7ee |
| segbits_rclk_dsp_intf_clkbuf_l.db | 288 | d57d1c375341d3271af12a37c72dbb5767a4e5ce653434b7956b30f3381b35c9 |
| segbits_rclk_dsp_intf_l.db | 80 | 55d609eeeaa64a2c7fbb3c9263942813e06b27da8dd499533384dc03edaae3f3 |
| segbits_rclk_dsp_intf_r.db | 80 | 1cba6fa3c1138ed9fc1fc3da5dbe9b7f6a58e8c9634e6e513a670c14570c7f23 |
| segbits_rclk_hdio.db | 512 | 8751a63675994a435963ff76dbd1ced4c599e1b3fdcfa4ea1797679c8143bd42 |
| segbits_rclk_int_l.db | 1576 | 3399543f2cbe0ceee71f70e07e561f193d3da312b6a00e6d99f305a546a38216 |
| segbits_rclk_int_r.db | 1576 | 2d003f44af8b29179dcdcbe9c5e9e393f8a9fb82ae70c81e7fe9ea7521ace3b1 |
| segbits_rclk_intf_left_term_alto.db | 456 | b290ed04243e9d2ce32146192416467c65623251ed6bd60b149855e35923b7a1 |
| segbits_rclk_xiphy_outer_right.db | 48 | ee9156d2939e5e1a0b21defa58b8e53520738d19de26f0a82489a4a0c4b5432d |

Total vendored segbits features: 15636.

The fifteen `segbits_rclk_*.db` files were vendored 2026-11-19 by the
clocking worker (branch `fm/ultra-bitgen-clk`), same source repo,
commit, and license as the nine above. They are the row-clock (RCLK)
tile encodings: the clock-distribution PIPs and USED bits for every
RCLK tile type, plus the only site-config encodings clocking has in
any public source, BUFCE_LEAF (in `segbits_rclk_int_l.db` and
`segbits_rclk_int_r.db`, 416 config lines each) and BUFGCE_HDIO IINV
(in `segbits_rclk_hdio.db`, 8 lines). `segbits_rclk_intf_left_term_alto.db`
is Zynq-PS-only (BUFG_PS PIPs, no PS on xcku3p/xcvu3p); it is vendored
for inventory completeness and must not be used for those parts. See
`CLK_AUDIT.md` for the per-file inventory and what is still missing
(BUFGCE/BUFGCTRL/MMCM/PLL site config: zero lines in any public source).

The four CLB DBs (CLEL_L, CLEL_R, CLEM, CLEM_R) were vendored 2026-11-07
by the emitter worker (branch `fm/ultra-bitgen-emitter`), same source
repo, commit, and license as the five above, because the emitter needs
the CLB encodings to emit LUT INIT and FF INIT for the golden designs.
Same 4-line header, same byte-identity rule, same verification. The
emitter derives the single-feature `X6LUT.INIT[63:0]` lists from the
per-bit `XLUT.INIT[i]` lines of these DBs at load time (see
`FileSegbitsDb::synthesize_lut_init` in `src/segbits.rs`); the derived
lists are not stored on disk.

### Origin-info DBs (`.origin_info.db` files)

Per-line provenance records shipped by prjuray-db (which fuzzer run
produced each line, e.g. `origin:050-int-seed`). Same 4-line header,
same verification rule.

| File | Upstream lines | SHA-256 of upstream data |
|---|---|---|
| segbits_int.origin_info.db | 3778 | dfd5fd737f9a6afb092a5ec4bebc8012cc6a3310137164a8b02044a5eed39179 |
| segbits_hpio_right.origin_info.db | 1560 | 6fc22620c37d7518453a56b4e29a901a1a1b2475434cafaa7e0d24e344d0c9c4 |
| segbits_hdio_top_right.origin_info.db | 812 | ae96512ab0b1744a2ed629d6512693b7f5b8c611f7cb10b2cb9df50d51edb950 |
| segbits_hdio_bot_right.origin_info.db | 828 | 57019d0dd3630949824becd7b92b2d3fb83738ba3e16d751e21f8739340a4d7f |
| segbits_cmt_right.origin_info.db | 104 | f0eb87f07a05fe69addc5ba51e2715d1b967f13a6446a6e7f0be4a909285bdb8 |
| segbits_clel_l.origin_info.db | 768 | a9c04251d7d78827e7ed1dfae2b7be1edcf5302b923b5a224043cf8cd7f8459f |
| segbits_clel_r.origin_info.db | 768 | 1571cef21927800cddc70100ab08e61209145736b619d7692001a3304d7be754 |
| segbits_clem.origin_info.db | 829 | 27a0ccaa65895a667104e8ecb0efab7d91987a14a9af78880766276eeba4a3dc |
| segbits_clem_r.origin_info.db | 829 | 41e75c70b73fa609aea24bed5e8778adb7aacf9a79a08cefcbe3583d058e054f |
| segbits_rclk_ams_cfgio.origin_info.db | 144 | 923ffcd80d71fa3e1b2b568034af0a621c72b41e66a33d5e00c5fb865d75e721 |
| segbits_rclk_bram_intf_l.origin_info.db | 160 | 5983f2bad5e544152003b352669311cca3f2160dfa286c19528a996b6b8f3226 |
| segbits_rclk_bram_intf_td_l.origin_info.db | 160 | 95ff6003db007cc2e560c9e8dce60188277ead53e6029ecd0a8f123172d81b54 |
| segbits_rclk_bram_intf_td_r.origin_info.db | 160 | aeb988995aaf5df2627c3d46d0dddefe6342d12b1cb510f9d1805b1a8fe4de06 |
| segbits_rclk_clel_l_l.origin_info.db | 40 | 1c2cc6402d010fefcacf07e86331e43473c8b5b21de13da5f57e97d21cb3e7df |
| segbits_rclk_clem_l.origin_info.db | 40 | eb742bd0489bda91f22ce1870b3a83fa5df741c3cc2d0859dc45cd1188eccd25 |
| segbits_rclk_clem_r.origin_info.db | 40 | f3390bd2b0ee07eb56e2a2b120eb52b8610a228cb62305648dee68d39e5d7822 |
| segbits_rclk_dsp_intf_clkbuf_l.origin_info.db | 288 | 28fa2dc1ffd80355cfb920339897bbaef4263879d21498ade3abbfb909dcc23c |
| segbits_rclk_dsp_intf_l.origin_info.db | 80 | ece0593f83b7180587c95bd0e6e821e399723e9234dc6f751ea098cb7d01b7f9 |
| segbits_rclk_dsp_intf_r.origin_info.db | 80 | 92771979efeb94cf602e975c53c5997d75014c118a81e14f4d72ed00920de63e |
| segbits_rclk_hdio.origin_info.db | 512 | a890ee3da8537a44828c63072c76ac16c9f52039a096e21d480315902ad789e4 |
| segbits_rclk_int_l.origin_info.db | 1576 | 9b8247bfb3a8ea7cacc9e0c258ae771383d54d5a320a07730bc1b8f12aec2101 |
| segbits_rclk_int_r.origin_info.db | 1576 | c6e52c0cce8272c776414a41e470465da6577466da6f6d32615505967c918da7 |
| segbits_rclk_intf_left_term_alto.origin_info.db | 456 | 394972d5a3dea429c2b654912d9f27d874f070e2bc63f7bdfbd7acb227d7a3dd |
| segbits_rclk_xiphy_outer_right.origin_info.db | 48 | 5e17e3b76d1c9fd6a25a8a062051c396fe7867a46d697ab919479dcc145188b4 |

### Tile-type and site-type JSONs

JSON has no comment syntax, so these files are byte-identical copies of
upstream with no inline header; provenance is recorded here. SHA-256
values below are of the vendored file itself (identical to upstream).

| File | Upstream path | SHA-256 |
|---|---|---|
| tile_type_INT.json | zynqusp/tile_types/tile_type_INT.json | 26101bb598889774d38c57e5905054d231640fac48ae0a133d016a6cd1f95459 |
| site_type_SLICEL.json | zynqusp/site_types/site_type_SLICEL.json | 13f855f50f5ae86b889481aaadc1aacae01a32883db6a3d1526780f940b477d1 |
| site_type_SLICEM.json | zynqusp/site_types/site_type_SLICEM.json | 4809539338e074a1010142b8746c5eba0a99cb0dc93b56095b9a25cef68021d5 |
| site_type_DSP48E2.json | zynqusp/site_types/site_type_DSP48E2.json | 63c1cdb567490a8d1dfe1665c01d40f301899488ed6cd9f0d9985846a9644a16 |
| site_type_BUFGCE.json | zynqusp/site_types/site_type_BUFGCE.json | b20d79436cb0161daf323117c73429c7b0b83fb862ed113ad5d1f14c0b529c1e |
| site_type_BUFGCTRL.json | zynqusp/site_types/site_type_BUFGCTRL.json | b243b3f606cc4c2aebdcc7624e8f12f69d977fbd2d369e3e8b151472e0659fcf |
| site_type_RAMB181.json | zynqusp/site_types/site_type_RAMB181.json | a8c378c0d7c686fafc9d12b763e10d2f8186cd9e32ed145f8a285c991d99d6be |
| site_type_RAMBFIFO36.json | zynqusp/site_types/site_type_RAMBFIFO36.json | 9c955c2968c0a576e3844fbf1dbb35c75921f0f403aaa3e9526cb040173f12a5 |
| site_type_HPIOB_M.json | zynqusp/site_types/site_type_HPIOB_M.json | a3161d8e2cf0a4ce4c7fb410637474bee0e320261ac9edc690c81d0f5c1d0b96 |
| site_type_HPIOB_S.json | zynqusp/site_types/site_type_HPIOB_S.json | 8b7db0f572edb3aaff02a3a8729412f8e3760ad8253fa60524c4dccdc5210cd4 |
| site_type_HDIOLOGIC_M.json | zynqusp/site_types/site_type_HDIOLOGIC_M.json | f8315cd3d1d5ea40933450e574d71e7c703195794b8b9f742c0355d59a856491 |
| site_type_HDIOLOGIC_S.json | zynqusp/site_types/site_type_HDIOLOGIC_S.json | ba6a1807335a7fc19c62bdd18eb76006ee0e2511c5a7694c27750d71a7d684e4 |
| site_type_MMCM.json | zynqusp/site_types/site_type_MMCM.json | bf460d10a0e5f70762440777d5d9ac34afc10013fb138d26e9bf7b577cfabea4 |
| site_type_PLL.json | zynqusp/site_types/site_type_PLL.json | 80daafa5813e1d93f76256907b2f34770503497dadbf33f0383a88bf49dc9231 |
| site_type_BUFCE_LEAF.json | zynqusp/site_types/site_type_BUFCE_LEAF.json | 77f6f1e624659d460dca62d98a6b95f25aac7fccdc7158255618e15a316ea280 |
| site_type_BUFCE_ROW.json | zynqusp/site_types/site_type_BUFCE_ROW.json | 7bc49f9cd70f16a904e1dc866593e660e86e660f7e34b15cbb55759f8517adcb |
| site_type_BUFCE_ROW_FSR.json | zynqusp/site_types/site_type_BUFCE_ROW_FSR.json | d2083be72b26291ff750cc8935cd84ef215da140254d49fdc4e1c01f9e86a2f9 |
| site_type_BUFGCE_DIV.json | zynqusp/site_types/site_type_BUFGCE_DIV.json | b856950327b69fe1147c8b0fef34c6f6cf25a264cffac7372872830ad75946e8 |
| site_type_BUFGCE_HDIO.json | zynqusp/site_types/site_type_BUFGCE_HDIO.json | fded3a6a71b5fad20137c9d1b422067e47a24f44dabd5ab8dfd8b64c9321eaf5 |

The five clock site-type JSONs were vendored 2026-11-19 by the
clocking worker (branch `fm/ultra-bitgen-clk`), same source repo,
commit, and license as the fourteen above. They are the pin/pip
inventories for the clock sites the RCLK encodings configure
(BUFCE_LEAF, BUFCE_ROW, BUFCE_ROW_FSR, BUFGCE_HDIO) and for
BUFGCE_DIV, whose site inventory exists but whose config-bit encoding
does not (see `CLK_AUDIT.md`).

Why these JSONs: `tile_type_INT.json` is the enumeration source for the
INT PIP completeness audit (`INT_PIP_AUDIT.md`). The site-type JSONs are
the site pin/pip inventories for the sites that live in the tiles whose
segbits are vendored here (HPIO/HDIO/CMT) plus the core CLB, DSP, and
BRAM sites the pipeline configures. The INT tile itself has zero sites,
so no site-type JSON feeds the INT audit directly.

## BitFiltrator (arch JSON pointers, not a vendored copy)

- Source repo: https://github.com/epfl-vlsc/bitfiltrator
- Reference clone commit: `79b9bd66ecae31354936143180fa578dc5732baf`
  (2023-04-08), at `~/workspace/helion-ultra-bitstream/bitfiltrator-ref/`
  (untracked in the base worktree; not present in this worktree).
- License: MIT. Verified 2026-11-07: the clone's `LICENSE` file is the
  MIT license, Copyright (c) 2022 EPFL VLSC.
- The 4.5 MB `resources/archs/ULTRASCALE_PLUS.json` is NOT copied here
  in full; the device-file worker owns the arch JSON. The four
  LUT/FF/BRAM bit-location tables extracted from it ARE vendored here
  as `ultrascale_plus_lut_ff_bram.json`; see the subsection below.
- The pointers this crew relies on, for the record:
  - LUT INIT: `{CLEL_L,CLEL_R,CLEM,CLEM_R}.LutLoc.Y_ofst[0..59].frame_ofst[{A..H}6LUT]` (64 offsets each) and `.minor[...]`
  - FF init/config: same file, `.RegLoc.Y_ofst[0..59].frame_ofst[{A..H}FF, {A..H}FF2]` and `.minor[...]` (16 FF names)
  - BRAM content: `BRAM.BramMemLoc.Y_ofst[0..23].frame_ofst` (16384 offsets) and `.minor[...]`
  - BRAM parity: `BRAM.BramMemParityLoc.Y_ofst[0..23].frame_ofst` (2048 offsets) and `.minor[...]`
  - FAR layout: `src/arch_spec.py`, class `UltraScalePlusSpec` (UG570 table 9-21)
- Pointers verified against the day-1 scout report
  (`~/workspace/helion-ultra-bitgen-segbits/report.md`).

### Vendored BitFiltrator tables (LUT / FF / BRAM bit locations)

Extracted verbatim from `resources/archs/ULTRASCALE_PLUS.json` by the
lut-ff-bram worker (branch `fm/ultra-bitgen-lut`, commit 0e81099; merged
to `fm/ultra-bitstream` as a09df12). Extraction details, table layout,
and cross-checks are in `README.md` in this directory; provenance is
recorded here, the single index for this directory.

| File | Source SHA-256 | License |
|---|---|---|
| ultrascale_plus_lut_ff_bram.json | c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c | MIT, Copyright (c) 2022 EPFL VLSC |
| generate_tables.py | extraction script, not vendored data | MIT |

Source SHA-256 is of `resources/archs/ULTRASCALE_PLUS.json` in the
BitFiltrator reference clone at commit
`79b9bd66ecae31354936143180fa578dc5732baf` (recomputed 2026-11-07
during the segbits-vendor rebase onto a09df12). The extracted tables
are byte-identical to the source sections; `generate_tables.py`
asserts the table counts and warns if the source hash changes.

### Vendored BitFiltrator device files

| File | Source SHA-256 | License |
|---|---|---|
| xcku3p.json | 07e6a7d3d586bf9ae3ad6c5d0959b669c5332af36f473ecd44e169c47512cfbe | MIT, Copyright (c) 2022 EPFL VLSC |
| xczu3eg.json | 248e0f9fb43187e893e1c2566ac59c781d6f1e424c8fae145afc3eb60a765863 | MIT, Copyright (c) 2022 EPFL VLSC |
| xczu3eg_int_columns.json | 5a88f87f8e7f18f369ea14a4ffca1c66ba5026a2d52b03461f8a85ebbd203fc2 | CC0-1.0 (derived from prjuray-db tilegrid) |

`xcku3p.json` and `xczu3eg.json` are vendored verbatim from
`bitfiltrator-ref/resources/devices/`; extraction details and
cross-checks are in `README.md` in this directory.
`xczu3eg_int_columns.json` is not a BitFiltrator file: it is the
Helion-derived INT column table for xczu3eg, generated from the
public prjuray-db tilegrid by
`tools/verify_xczu3eg.py --write-int-columns` (see README.md for
the derivation and the 5,940-of-5,940 verification).

## Helion-derived segbits (not upstream data)

The files in this section are derived by Helion from the public
corpus bitstreams, not vendored from prjuray-db. They are kept
separate from the vendored table above: no upstream line count or
SHA-256 applies, and their headers say so. Each file documents its
derivation and cross-validation in its own header; the lane report
holds the full evidence.

| File | Lines | Derived | Evidence |
|---|---|---|---|
| segbits_int_intf_r_pcie4.db | 2 | 2026-11-22, INT_INTF lane | OUTPUTS_ENABLED.1 = 01_21 02_27 03_24, identical in 5 independent corpus designs (P-13..P-17), 8 tiles each, 120 bits total; positions match INT_INTF_LEFT_TERM_PSS.OUTPUTS_ENABLED.1 in the public segbits_int_intf_left_term_pss.db exactly. No INT_INTF PIP bit is set in any corpus design, so no PIP line is derived; see ultra-bitgen-crew/INTINTF_REPORT.md. |

## What is NOT here (rejected or missing)

- RapidWright `.dat` files: REJECTED (Xilinx EULA). Never ingested.
- FPGA Interchange DBs: REJECTED (Xilinx EULA). Never ingested.
- DSP48E2 config-bit encoding: MISSING in both public sources
  (site_type_DSP48E2.json documents pins only). Fuzzing plan is in the
  day-1 report; DSP48E2 lookups fail loud until it lands.
- URAM288: MISSING in both public sources. Fail loud until it lands.
- BUFGCE / BUFGCTRL / BUFGCE_DIV / MMCM / PLL site config-bit encoding:
  MISSING in every public source checked 2026-11-19. Zero lines in
  `segbits_cmt_right.db` (104 lines, HROUTE/HDISTR only), zero lines in
  all fifteen vendored `segbits_rclk_*.db` files (5,360 lines), zero
  entries in BitFiltrator `ULTRASCALE_PLUS.json` (top-level keys BRAM,
  CLEL_L, CLEL_R, CLEM, CLEM_R only). The site-type JSONs document pins
  and inversion pips only, no bits. Clocking designs using these sites
  fail loud (`ClkError::MissingEncoding` in `src/clk.rs`) until the
  Vivado fuzzing job in `CLK_FUZZ_JOB.md` lands. The encodable clocking
  subset (CMT HROUTE/HDISTR, RCLK PIPs and USED bits, BUFCE_LEAF config,
  BUFGCE_HDIO IINV) is vendored above and implemented in `src/clk.rs`;
  see `CLK_AUDIT.md` for the exact inventory.
- xcvu3p device file: MISSING from BitFiltrator (it ships xcku3p
  and xczu3eg device files; no xcvu3p file). The xcvu3p.json in
  data/ is a script-built provisional file (see README.md), not a
  BitFiltrator vendoring.
