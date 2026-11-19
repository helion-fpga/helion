# UltraScale+ device-model gap register (GAP-F7)

Worker: device model, session 18. Branch `fm/ultra-bitgen-devmodel`.
Date: 2026-11-19. No Vivado was run, no license check was run, no
bitstream was claimed. Every number below was produced by the
commands in the Evidence section, run against the files in this
repo and the reference clones in the base worktree.

## Sources used (all public, CC0 or MIT)

- AMD UG570 (UltraScale Architecture Configuration User Guide),
  Table 1 (configuration frames) and the FAR table, read from
  docs.amd.com 2026-11-19: KU3P and KU5P 41,476 frames each; VU3P
  71,820; AU20P and AU25P 41,476 each; 93 words per UltraScale+
  frame. Valid FAR block types are 000 (CLB, I/O, CLK) and 001
  (BRAM content); a normal bitstream does not include 010, 011,
  or 100. IDCODEs: KU3P X4A63093 (revision 0 or later), KU5P
  X4A62093, VU3P X4B39093 (revision 1 or later).
- AMD DS890 Table 9 (v4.10, May 21, 2026): KU3P CLB LUTs 162,720,
  CLB FFs 325,440, Block RAM Blocks 360, DSP Slices 1,368; VU3P
  CLB LUTs 394,080, CLB FFs 788,160, Block RAM Blocks 720,
  UltraRAM Blocks 320, DSP Slices 2,280. DS890 publishes no slice
  count for any part.
- AMD XMP103 v2.8: KU3P CLB LUTs 163K, FFs 325K, DSP 1,368,
  BRAM 12.7 Mb, URAM 13.5 Mb; VU3P CLB LUTs 394K, FFs 788K,
  DSP 2,280, BRAM 25.3 Mb, URAM 90.0 Mb. XMP103 publishes no
  slice count either.
- BitFiltrator (MIT, EPFL VLSC), reference clone in the base
  worktree: `resources/devices/*.json` (40 files),
  `resources/parts_all.json`, `resources/parts_webpack.json`,
  `src/create_device_summary.py` (header counts are Vivado part
  properties: `num_slices` is `SLICES`, `num_luts` is
  `LUT_ELEMENTS`, `num_regs` is `FLIPFLOPS`, `license` is
  `LICENSE` with values Webpack or Full),
  `src/extract_all_frame_addresses.py` (FARs come from a
  per-frame-CRC debug bitstream; FAR writes to a reserved
  address are dropped, duplicates are removed),
  `src/extract_num_majors_and_minors.py` (minors tables are
  derived from that FAR list).

## Gap inventory

### G2. device.rs 144x400 grid is invented, not ground truth

Status: VERIFIED and pinned. The grid in `src/device.rs`
(`UltraScaleDevice::xcvu3p`, 144 wide x 400 tall) disagrees
outright with the device file (xcku3p: 113 CLB columns per row,
4 FAR rows), and bitgen-core does not consume it: `emit_bitstream`
takes `&dyn BitgenDevice`, implemented by `DeviceFile`
(`src/bitgen_device.rs`), never by `UltraScaleDevice`, and
`tile_base_far` exists only on that trait. Regression test
`far_mapping_comes_from_device_file_not_logical_grid` in
`tests/device_model_counts.rs` pins the grid/file disagreement
and the file-derived FAR anchor (SLICE_X23Y45 base 0x3000,
BFF minor 12, A6LUT bit 0 minor 11). No code change was needed;
the separation already held and is now tested.

Open remainder (not bitgen): pack/place still consume the
invented grid through `src/grid.rs` (`UltraGrid`), with
provisional tile names (`CLBLL_L_X{x}Y{y}`). That is a
placement-model limitation, owned by the integration lane, not
fixed here because changing it would change placement
semantics. Unblocker: a real tilegrid (Vivado probe family,
PinPlanning probe) or a device-file-driven grid.

### (b1). xcku3p slices: 20,340 usable vs 27,120 sites

Status: RECORD CORRECTED, both counts grounded, tests pinned.
The old record called the file's 27,120 a "file-header anomaly".
That is refuted: in all 40 BitFiltrator device files, header
`num_slices` equals geometry slices (CLB columns x FAR rows x
60), and `create_device_summary.py` shows it is the Vivado
SLICES part property. 20,340 is LUT-derived from the
AMD-published 162,720 LUTs (162,720 / 8 = 325,440 / 16 =
20,340); AMD publishes no slice count, and the distributor
figure 20,340 is the same derivation. The two differ by exactly
one clock row (6,780 = 113 x 60). KU3P and KU5P share identical
file geometry (rowMajors byte-identical) and the identical UG570
frame count (41,476), while KU3P usable LUTs, DSPs, and BRAMs
are exactly 3/4 of KU5P (162,720 vs 216,960; 1,368 vs 1,824;
360 vs 480). The file geometry is the shared-die geometry.
DSP header 1,368 is exactly 18 per DSP column (76 columns);
BRAM header 360 is exactly 9 per BRAM column (40 columns).
Neither number was changed to match the other. See
`tests/device_model_counts.rs`.

Open remainder: which sites are usable on KU3P (the fused-off
subset of the shared die) is not in any public source.
Unblocker: Vivado probe, `get_sites SLICE` on
xcku3p-ffva676-1-e (PinPlanning probe family), plus the
per-site usable LUT/DSP/BRAM properties.

### (b2). xcvu3p slices: 49,250 vs 49,260

Status: CLOSED before this session, verified here. No 49,250
remains in the crate; the model uses 49,260 = 394,080 / 8
(DS890 Table 9 exact LUT count, CTO ruling 2026-11-07; 49,250
came from dividing the rounded 394,000). Stale 49,250
references in `scripts/gen_xcvu3p.py` and the xcvu3p.json
`num_slices` note are fixed in this session (commit 98a2ce1).
Regression test pins 49,260 x 8 = 394,080 and x 16 = 788,160,
and the provisional geometry (821 CLB columns x 60) matching
the header. Vivado confirmation must still re-check the count.

### (c). xcku3p frame count: file 41,460 vs UG570 41,476

Status: OPEN, verified, narrowed, not closable from public
sources. The gap is 16 frames, exactly 4 per FAR row, and the
4-per-row pattern is confirmed on four devices whose UG570
counts were checked (xcku3p, xcku5p, xcau20p, xcau25p: UG570
41,476 each, file 41,460 each, 4 FAR rows each). The
extraction source is a per-frame-CRC debug bitstream's FAR
writes, which drop reserved-address FAR writes and
deduplicate, so the 16 frames are either missing from that
extraction or counted by UG570 outside the FAR-addressable
minors. Block type 2 is excluded as the cause (UG570 FAR
table: a normal bitstream does not include block types 010,
011, or 100). The loader reports the file total and documents
the delta; the tables were not tuned. Regression test pins
`total_frames() + 4 * rows == 41_476`.

Unblocker: a fresh Vivado KU3P debug bitstream (per-frame
CRCs) run through BitFiltrator
`extract_all_frame_addresses.py`, plus an FDRI word-count
cross-check (FDRI words / 93 must equal 41,476), locating
the 16 FARs by row and column. Needs Vivado plus the license
server, both blocked since Oct 22, 2026.

### (d). Every provisional value in xcvu3p.json

Status: FLAGGED, one false source corrected, the rest open.
The file carries 18 provisional notes and `provisional: true`.
Verified in this session:

- `part` xcvu3p-ffvc1517-1-e: real part, listed in
  BitFiltrator `resources/parts_all.json` (Virtex UltraScale+
  xcvu3p). Grounded.
- `slrs.SLR0.idcode` 0x14B39093: UG570 Table 2, X4B39093,
  production revision 1 or later. Grounded as a UG570 value;
  still flagged until Vivado confirms the file.
- `num_slices` 49,260, `num_luts` 394,080, `num_regs` 788,160,
  `num_brams` 720, `num_dsps` 2,280: DS890 Table 9 exact
  values (XMP103 rounded). Grounded as AMD-published header
  counts; the geometry they sit on is not confirmed.
- `license` Webpack: CORRECTED in this session (commit
  98a2ce1). The note used to claim "xcvu3p appears in
  parts_webpack.json". False: that file lists NO xcvu3p parts
  (Virtex UltraScale+ xcvu3p: []). All 40 BitFiltrator device
  files carry Webpack, but that is the template's value, not
  evidence about xcvu3p. The Vivado LICENSE property for
  xcvu3p-ffvc1517-1-e is unknown. The value stays Webpack as
  an unconfirmed template copy; it must not be presented as
  sourced. Regression test pins the corrected note.
- `max_far_row_idx` 6 (7 FAR rows), `max_clock_region_row_idx`
  6, `max_clock_region_col_idx` 3, all `rowMajors` column
  tables (821 CLB columns, 19 DSP columns and 10 BRAM columns
  per row copied from the xcku3p template), and the frame
  total 73,035: construction, not measurement. The total
  overshoots UG570 Table 1 (71,820) by 1,215, the opposite
  direction from xcku3p. The tables were not tuned to match.
  The header-vs-geometry gaps are pinned in tests (tables
  imply 3,192 DSP sites vs header 2,280, and 840 BRAM sites
  vs header 720).

Unblocker for the whole file: Vivado confirmation
(`create_device_summary.py`, plus a Vivado xcvu3p bitstream
for the FAR tables), blocked on the license server since
Oct 22, 2026. The exact probe per field is the Vivado part
property (LICENSE, SLICES, LUT_ELEMENTS, FLIPFLOPS) and the
per-frame-CRC debug-bitstream FAR enumeration.

### (e). URAM sites are not in the device-file schema (found while verifying)

Status: OPEN, recorded. The device-file schema has no URAM
column tables, so URAM288 FAR mapping is unmodeled for both
parts (VU3P 320 blocks, KU3P 48 blocks per DS890). No number
was invented. Unblocker: the same Vivado FAR enumeration as
(c), extended to the URAM columns, or a BitFiltrator schema
extension with URAM tables.

## What was verified byte-identical

- `crates/helion-ultrascale/data/xcku3p.json` is byte-identical
  to `bitfiltrator-ref/resources/devices/xcku3p.json`
  (SHA-256 07e6a7d3d586bf9ae3ad6c5d0959b669c5332af36f473ecd44e169c47512cfbe,
  `cmp` clean). Do not edit it; corrections go through the
  BitFiltrator source.
- No xcvu3p.json exists in the BitFiltrator reference clone;
  the repo file is entirely Helion-generated
  (`scripts/gen_xcvu3p.py`).

## Evidence (commands and key outputs)

All commands run 2026-11-19 in
`~/workspace/helion-ultra-bitgen-devmodel` unless noted.

Frame and geometry sums (python3 over the vendored JSON):

```
xcku3p header: slices=27120 luts=162720 regs=325440 dsps=1368 brams=360 rows=4
 row 0..3: clb_cols=113 dsp_cols=19 bram_cols=10 std_minors_sum=7805 bram_minors_sum=2560 row_total=10365
 TOTAL frames std=31220 bram=10240 sum=41460
 implied: clb_cols_total=452 slices=27120; dsp_cols_total=76; bram_cols_total=40
xcvu3p header: slices=49260 luts=394080 regs=788160 dsps=2280 brams=720 rows=7
 row 0,6: clb_cols=118 std=7885; rows 1..5: clb_cols=117 std=7869; bram 2560 each
 TOTAL frames std=55115 bram=17920 sum=73035
 implied: clb_cols_total=821 slices=49260; dsp_cols_total=133; bram_cols_total=70
```

All-40-files check (BitFiltrator reference clone): header
`num_slices` equals geometry slices (CLB columns x FAR rows x
60) in every file, including multi-SLR files summed over all
SLRs (xcu200: 147,780 both; xcu250: 216,000 both).

KU3P vs KU5P (reference clone):

```
KU3P vs KU5P rowMajors identical: True
KU3P vs KU5P tileType pairs identical: True
headers KU3P: num_slices 27120, num_luts 162720, num_regs 325440, num_dsps 1368, num_brams 360
headers KU5P: num_slices 27120, num_luts 216960, num_regs 433920, num_dsps 1824, num_brams 480
idcodes: 0x04a63093 0x04a62093
```

parts_webpack.json (reference clone):

```
webpack Virtex UltraScale+ xcvu3p: []
webpack Kintex UltraScale+ xcku3p: ['xcku3p-ffva676-1-e', ... 36 parts]
```

License values across all 40 reference device files:
`Counter({'Webpack': 40})`.

UG570 Table 1 and Table 2, read from docs.amd.com
2026-11-19: KU3P 41,476 frames, KU5P 41,476, AU20P 41,476,
AU25P 41,476, VU3P 71,820, 93 words per frame; KU3P IDCODE
X4A63093 revision 0 or later, VU3P X4B39093 revision 1 or
later. Bitstream-length check: 41,476 x 93 x 32 + 515 x 32 =
123,449,056 bits, matching the published KU3P bitstream
length exactly.

DS890 Table 9 (v4.10), read 2026-11-19: KU3P System Logic
Cells 355,950, CLB FFs 325,440, CLB LUTs 162,720, Block RAM
Blocks 360, DSP Slices 1,368; VU3P System Logic Cells 862,050,
CLB FFs 788,160, CLB LUTs 394,080, Block RAM Blocks 720,
UltraRAM Blocks 320, DSP Slices 2,280.

Generator regeneration (`python3 gen_xcvu3p.py`, then
`--check`): slices 49,260, frames 73,035 (UG570 71,820,
overshoot 1,215), provisional notes 18, self-checks PASS.
Regenerated diff vs the previous file is exactly the two
corrected note sources (license, num_slices).

## Commits on fm/ultra-bitgen-devmodel (this session)

- eb2af3e: fix(ultrascale): correct xcku3p slice-count record,
  pin count cross-checks (device.rs, device_file.rs docs,
  data/README.md, tests/device_model_counts.rs).
- 98a2ce1: fix(ultrascale): correct xcvu3p license note, drop
  stale 49,250 text (gen_xcvu3p.py, regenerated xcvu3p.json,
  tests/device_file_xcvu3p.rs regression test).
- (this file) device-model gap register, GAP-F7.

No merges, no pushes, no Vivado runs, no license-port checks.
ZedBoard untouched (out of scope).
