# Vendored data for crates/helion-ultrascale

Two groups of files live here: the BitFiltrator bit-location tables
(LUT / FF / BRAM) and the device files (xcku3p control, xcvu3p
provisional). `PROVENANCE.md` (segbits-vendor lane) is the single
provenance index for this directory; this README carries the
extraction details for the tables and device files below.

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

- No LUTRAM table exists in the source JSON, and the BitFiltrator paper
  section V-B does not state that LUTRAM uses the same INIT bits as
  LUTs (it describes LUTRAM/register formats as discovered separately
  from the logic location file, while standard LUTs were fuzzed). LUTRAM
  emission is therefore blocked, fail loud, until a LUTRAM table or
  Vivado logic-location data lands. See `src/bitloc.rs`.
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
4 per FAR row, and it is systematic across every BitFiltrator device
file checked (xcku5p, xcu200, xcu250, xcu280 all show 4 missing
frames per row against UG570). The cause is unknown: either 4 frames
per row exist that BitFiltrator's extraction does not cover, or UG570
counts 4 frames per row outside the FAR-addressable array. Open
question for the CTO, not a loader defect: the loader reports the
file's own total and documents the UG570 delta.

Second cross-check, same status: the header says 162,720 LUTs and
325,440 FFs for 27,120 slices, exactly 6 LUTs and 12 FFs per slice,
3/4 of the 8-LUT / 16-FF arch geometry (LutLoc has 8 names, RegLoc
16). Both header values come from Vivado part properties via
create_device_summary.py and are reported as file truth, never
derived from geometry; the loader does not assert num_luts ==
num_slices * 8. Cause unknown (Vivado part-property counting vs
physical sites). For xcvu3p, the XMP103-published 394,080 LUTs /
788,160 FFs over 49,260 slices follow 8/16, so the provisional file
uses those; Vivado confirmation must re-check.

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
of assumptions, including the 49,250 vs 49,260 CLB discrepancy and the
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
