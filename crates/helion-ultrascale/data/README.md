# Vendored data for crates/helion-ultrascale

Two groups of files live here: the BitFiltrator bit-location tables
(LUT / FF / BRAM) and the device files (xcku3p control, xcvu3p
provisional, xczu3eg proving part). `PROVENANCE.md` (segbits-vendor
lane) is the single provenance index for this directory; this README
carries the extraction details for the tables and device files below.

## BitFiltrator tables (LUT / FF / BRAM bit locations)

`ultrascale_plus_lut_ff_bram.json`: the four bit-location tables the
lut-ff-bram worker consumes, extracted verbatim from the BitFiltrator arch
JSON. No value was changed, added, or recomputed. The extraction is
reproducible with `generate_tables.py`.

- `LutLoc`, `RegLoc` for all four CLB tile types (`CLEL_L`, `CLEL_R`,
  `CLEM`, `CLEM_R`): 60 `Y_ofst` entries each. Per Y: 8 LUTs with 64
  (`minor`, `frame_ofst`) pairs each, 16 FFs with one pair each.
- `BramMemLoc`: 24 `Y_ofst` entries, 16384 (`minor`, `frame_ofst`) pairs
  each. `BramMemParityLoc`: 24 `Y_ofst` entries, 2048 pairs each.

Provenance:

- Source file: `bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json`
  (BitFiltrator, EPFL VLSC).
- Source SHA-256:
  `c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c`
- License: MIT, Copyright (c) 2022 EPFL VLSC. Full text in the
  BitFiltrator clone (`bitfiltrator-ref/LICENSE`); the MIT permission
  notice covers this copy.
- Extracted: 2026-11-07, with `generate_tables.py`, which asserts the
  table counts above and warns if the source hash changes.
- This file is the per-table extraction record. The directory-level
  provenance index is `PROVENANCE.md` in this directory; it records the
  same source SHA-256 and license for these tables alongside the prjuray
  segbits DBs, and points back here. If the source hash changes, update
  both files.

### Reading the tables

`minor` is the minor address of the configuration frame holding the bit;
`frame_ofst` is the bit offset within that 93-word frame. BitLocator in
the BitFiltrator reference (`src/bit_locator.py`, `src/arch_summary.py`)
is the authority for the lookup order:

- LUT: list index is the INIT bit index (0..63). `y_ofst` is the slice Y
  modulo 60 (clock-region height, `ArchSpec.num_clb_per_column`).
- FF: one (`minor`, `frame_ofst`) pair per FF name (`AFF`..`HFF2`).
  BitFiltrator names FFs `AFF`, Helion BELs name the same storage
  element `AQ` (UG574 Q output pin); the mapping is AFF=AQ, AFF2=AQ2,
  and so on through HFF2=HQ2.
- BRAM: list index is the absolute content bit, `INIT` word * 256 + bit
  (see `src/u-ray_bram_comparator.py`: `abs_idx = init_idx * 256 +
  bit_idx`). 16384 content bits are `INIT_00`..`INIT_3F` of one RAMB18;
  2048 parity bits are `INITP_00`..`INITP_07`. `y_ofst` is the RAMB18 Y
  modulo 24 (`ArchSpec.num_18k_bram_per_column`).

### What is NOT here

- No separate LUTRAM table exists in the source JSON, because none is
  needed: the LUTRAM content bits ARE the LUT INIT bits (`LutLoc`),
  proven three ways (see docs/ultra-bitgen-crew/subproblems/
  US-04-lutram-table.md). BitFiltrator's extraction code states the
  INIT bits "ended up having the same encoding for LUTRAMs and
  standard LUTs"; the vendored tables are byte-identical across all
  four CLB tile types; every prjuray xLUT.INIT[i] line equals the
  LutLoc Y0 pair. The mode-selection bits (MODE.RAM, DI1, SIZE,
  WA7/8/9USED) resolve through the vendored segbits_clem.db and
  segbits_clem_r.db. LUTRAM emission works on SLICEM tiles and fails
  loud on SLICEL tiles. See `src/bitloc.rs`. The paper section V-B
  claim stays rejected: the derivation does not rest on it.
- No INT PIP, IO, clock, DSP, or URAM encodings. Those belong to other
  workers.
- The `y_ofst` tables are per-column-relative. Absolute frame addresses
  also need the device file (row major, column major, SLR), owned by the
  device-file worker.

### Cross-checks performed at extraction time

- `LutLoc` and `RegLoc` are byte-identical across all four CLB tile
  types in the source file.
- The frame offsets are NOT a clean 48-per-Y formula: they jump by an
  extra 96 at Y_ofst 30 (clock-region boundary). Tables are vendored in
  full for that reason; do not replace them with a formula.
- Paper cross-check: section V-C works `SLICE_X136Y247/A6LUT` bit 0 as
  minor 11, frame offset 351. The table gives the same pair at
  `Y_ofst` 7 (247 mod 60), A6LUT, bit 0.

## Device files

### xcku3p.json

Complete Kintex UltraScale+ device file from BitFiltrator (EPFL VLSC,
FPL 2022), MIT license, Copyright (c) 2022 EPFL VLSC. Source:
`bitfiltrator-ref/resources/devices/xcku3p.json` in the base worktree
(~/workspace/helion-ultra-bitstream/bitfiltrator-ref/, untracked
reference clone, LICENSE verified MIT by critic-interfaces 2026-11-07).
Control part for the bitgen crew: the pipeline must go green on
xcku3p first, where this file plus demo.py reconstruction can validate
it. Do not edit: any correction goes through the BitFiltrator source.

Known cross-check, recorded not hidden: summing
num_minors_per_std_colMajor plus num_minors_per_bram_content_colMajor
over all rows gives 41,460 frames, while AMD UG570 Table 1 lists
41,476 configuration frames for KU3P. The gap is 16 frames, exactly
4 per FAR row. It is systematic: the same 4-per-row gap holds on
every BitFiltrator device file whose UG570 Table 1 count was checked
2026-11-19 (xcku3p, xcku5p, xcau20p, xcau25p: UG570 41,476 each, file
41,460 each, 4 FAR rows each). The extraction source is a
per-frame-CRC debug bitstream's FAR writes (BitFiltrator
`extract_all_frame_addresses.py`, which drops FAR writes to a
reserved address and deduplicates), so the 16 frames are either
missing from that extraction or counted by UG570 outside the
FAR-addressable minors; block type 2 is excluded as the cause
(UG570 FAR table: a normal bitstream does not include block types
010, 011, or 100). Open question for the CTO, not a loader defect:
the loader reports the file's own total and documents the UG570
delta. Unblocker: a fresh Vivado KU3P debug-bitstream FAR
enumeration with an FDRI word-count cross-check, locating the 16
FARs.

Second cross-check, resolved 2026-11-19 (see
tests/device_model_counts.rs): the header says 162,720 LUTs and
325,440 FFs for 27,120 slices, exactly 6 LUTs and 12 FFs per slice,
3/4 of the 8-LUT / 16-FF arch geometry (LutLoc has 8 names, RegLoc
16). The two counts measure different things and both are grounded:
27,120 is the Vivado SLICES part property
(`create_device_summary.py`: `summary["num_slices"] =
part_properties["SLICES"]`) and equals the file geometry exactly
(113 CLB columns x 4 FAR rows x 60 slices); in every BitFiltrator
device file checked, header num_slices equals geometry slices.
20,340 is the usable-slice count, LUT-derived from the AMD-published
162,720 LUTs (DS890 Table 9, XMP103 163K); AMD publishes no slice
count for KU3P. KU3P and KU5P share the identical file geometry
and the identical UG570 Table 1 frame count (41,476), while KU3P
usable LUTs, DSPs, and BRAMs are exactly 3/4 of KU5P (162,720 vs
216,960; 1,368 vs 1,824; 360 vs 480): the file geometry is the
shared-die geometry, and which sites are usable on KU3P is not in
any public source. The loader does not assert num_luts ==
num_slices * 8. For xcvu3p, the XMP103-published 394,080 LUTs /
788,160 FFs over 49,260 slices follow 8/16, so the provisional file
uses those (49,260 = 394,080 / 8; the earlier 49,250 came from
dividing the rounded 394,000 and is gone from the model); Vivado
confirmation must re-check.

### xczu3eg.json

Complete Zynq UltraScale+ device file from BitFiltrator (EPFL VLSC,
FPL 2022), MIT license, Copyright (c) 2022 EPFL VLSC. Source:
`bitfiltrator-ref/resources/devices/xczu3eg.json` in the base worktree
(~/workspace/helion-ultra-bitstream/bitfiltrator-ref/, untracked
reference clone). Vendored verbatim, byte-identical to the source;
SHA-256 `248e0f9fb43187e893e1c2566ac59c781d6f1e424c8fae145afc3eb60a765863`.
Part `xczu3eg-sbva484-1-e`, the proving part for the US-09 end-to-end
flow: it is the only UltraScale+ part with a public prjuray-db
tilegrid (CC0), so every table in the file was verified against
that tilegrid plus `part.json` by `tools/verify_xczu3eg.py`
(35 of 38 checks pass; report in `~/workspace/us09-device/`).
Do not edit: any correction goes through the BitFiltrator source.

Verified exactly against the tilegrid:
- Minors tables equal `part.json` frame counts in all 3 rows, both
  buses. Header counts equal tilegrid site counts: 8,820 slices
  (5,220 SLICEL + 3,600 SLICEM), 70,560 LUTs, 141,120 FFs, 216
  BRAMs, 360 DSPs. Unlike xcku3p, header slices equal geometry
  slices here (49 CLB columns x 3 FAR rows x 60); DS891 ZU3EG
  agrees (70,560 LUTs, 141,120 FFs, 7.6 Mb BRAM, 360 DSP, no
  UltraRAM). There is no KU3P-style usable-vs-geometry fraction.
- `clb_colMajors` / `clb_tileTypes` equal the tilegrid derivation
  in all rows, zero conflicts. `bram_content_colMajors` are
  {0:0..5:5}, parity equals content, `bram_reg_colMajors` are
  {0:6, 1:24, 2:37, 3:64, 4:70, 5:95}. `dsp_colMajors` are
  {0:11, 1:42, 2:57, 3:82, 4:88}, each the neighbour CLEM/CLEM_R
  column minus 1. `tileType_siteType_pairs` match the tilegrid
  58/58 (base types). Total frames 14,952 equals `part.json`.
- IDCODE 0x04A42093, confirmed by real Vivado bitstreams
  (big-endian bytes at offset 291 of the 14 public xczu3eg pairs).
  The AMD forum figure 0x14710093 is the PS TAP IDCODE, not the
  PL configuration IDCODE written in the bitstream.

Known cross-checks, recorded not hidden:
- UG1085 Table 1 (PL Bitstream Length) lists 14,964 configuration
  frames for ZU3; the file totals 14,952, gap 12 = 4 per FAR row x
  3 rows. This is the same systematic pattern as xcku3p (16 = 4 x
  4) and is explained, not a file defect: real bitstreams carry
  14,952 real frames plus 12 padding frames in the FDRI packet
  (decoder lane: `fdri_frames=14964 padding_skipped=12`). The
  loader reports the file's own total and documents the UG1085
  delta. `tests/device_file_xczu3eg.rs` pins 14,952.
- 12 (row, column) pairs own frames in `part.json` but have no
  owning tile in the tilegrid: row 0 cols 31, 77, 100; row 1 cols
  31, 75, 76, 77, 100; row 2 cols 31, 75, 77, 100 (4 frames each,
  10 at row 1 col 76). They sit in INT_INTF grid columns whose
  tiles carry empty `bits` entries. This is a tilegrid owner gap,
  not a device-file defect: the frames exist in the file, in
  `part.json`, and in real bitstreams, and no segbits feature
  references them, so emit/parse are unaffected.
- INT columns do NOT come from the device file: the file keys CLB
  columns by SLICE_X, and on xczu3eg the INT tile X is not the
  SLICE_X (tile X is per-type numbering; CLEL_L_X5Y0 holds
  SLICE_X7Y0). The generic provisional rule in `bitgen_device.rs`
  therefore matches only INT_X0 and INT_X1 (360 of 5,940 INT
  tiles) and mispredicts the rest. The emitter uses the
  tilegrid-derived sidecar `xczu3eg_int_columns.json` instead:
  (FAR row, INT tile X) -> CLB_IO_CLK column major, 99 entries
  (33 INT columns x 3 rows), generated by
  `tools/verify_xczu3eg.py --write-int-columns` from the INT
  tiles' own baseaddrs and verified 5,940 of 5,940 against the
  same-tile-X CLE neighbour rule (INT col = left CLE col + 1 =
  right CLE col - 1). The loader attaches it only for xczu3eg
  (part-checked); a table for another part fails loud.
- HPIO tiles exist in the tilegrid (e.g. HPIO_RIGHT_X32Y0,
  baseaddr 0x00006700, col 103, 8 frames) but do NOT resolve:
  no HPIO column mapping exists in the device file or the sidecar,
  and the xcku3p HPIO bridge (base FAR 0x000) is never applied to
  this part. Any HPIO line fails loud. Unblocker: a tilegrid-derived
  HPIO column table, the same pattern as the INT table.

### ULTRASCALE_PLUS.json

Architecture summary from BitFiltrator, same MIT source:
`bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json`. Per tile type
(CLEL_L, CLEL_R, CLEM, CLEM_R) it maps every LUT (A6LUT..H6LUT) and
every FF (AFF..HFF2) at every Y offset 0..59 to its (minor,
frame_ofst) pairs, plus BRAM content (16,384 bits) and parity (2,048
bits) locations per Y offset 0..23. Shared by every UltraScale+ part;
the device file selects the tile type and column, this file selects
the bits inside the frame. Do not edit.

### xcvu3p.json

PROVISIONAL. Built by the device-file worker from the xcku3p structure
above plus AMD XMP103 counts, by the generator script
`scripts/gen_xcvu3p.py` (kept next to this file in the repo, see its
header for the construction). Every value not copied from a confirmed
source is flagged in the file's `helion_provisional` object with its
source. The file is self-consistent (every FAR maps to a real tile
inside the file) but it is NOT confirmed: confirmation needs Vivado
(`create_device_summary.py`, ~4 runs), blocked on the license server
since Oct 22, 2026. Nothing provisional in this file may be presented
as confirmed. See the generator script and COORD.md for the full list
of assumptions, including the resolved 49,250 vs 49,260 slice count
(the model now uses 49,260 = 394,080 / 8; 49,250 came from dividing
the rounded 394,000) and the
frame-total gap against UG570: the generator computes 73,035 frames,
which OVERSHOOTS the UG570 Table 1 count of 71,820 by 1,215 (1.7%).
That is the opposite direction from xcku3p (16 under) and is recorded
as evidence about the construction, not tuned away: the 7-row count
may be wrong, the per-row std table may over-count IO columns for
VU3P, or UG570 Table 1 may count differently. This supersedes the
71,664 anticipation in the first draft of this section, which no
rows x (std + BRAM) construction with these tables reproduces.
`tests/device_file_xcvu3p.rs` pins the computed total so any
generator change fails loudly here.
