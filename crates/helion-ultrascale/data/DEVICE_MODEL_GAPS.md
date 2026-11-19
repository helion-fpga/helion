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
60), counting rows that carry no `clb_colMajors` key as zero
CLB columns (xcku025 rows 3-4 have minors tables but no CLB
columns; its header 18,180 still equals 101 x 3 x 60), and
`create_device_summary.py` shows it is the Vivado
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

Grounding refinement (replacement worker, 2026-11-19): the
full-density multipliers used for the implied-site counts
are not equally sourced. DSP 24 sites per column is grounded
in the file itself: every row of xcku3p.json, xcku5p.json,
and the generated xcvu3p.json carries
`min_dsp_y_ofst`/`max_dsp_y_ofst` spanning 0..23, i.e. 24
DSP sites per column, and KU3P usable 18 is 3/4 of that.
BRAM 12 sites per column has no such file-internal span
(the row records carry no BRAM y-offset range); 12 comes
only from the KU5P ratio (480 BRAMs over 40 columns) plus
the same 3/4 pattern (KU3P usable 9 = 3/4 of 12). The
xcvu3p implied counts (133 cols x 24 = 3,192 DSP sites,
70 cols x 12 = 840 BRAM sites) therefore rest on a
file-grounded DSP multiplier and a ratio-derived BRAM
multiplier; the test pins both so a generator change fails
loudly, but only the DSP one is independently file-grounded.

Open remainder: which sites are usable on KU3P (the fused-off
subset of the shared die) is not in any public source.
Unblocker: Vivado probe, `get_sites SLICE` on
xcku3p-ffva676-1-e (PinPlanning probe family), plus the
per-site usable LUT/DSP/BRAM properties.

### (b2). xcvu3p slices: 49,250 vs 49,260

Status: CLOSED before this session, verified here. No 49,250
remains in the crate as a live count or discrepancy (the
remaining mentions are historical: "the earlier 49,250 came
from dividing the rounded 394,000"); the model uses 49,260
= 394,080 / 8 (DS890 Table 9 exact LUT count, CTO ruling
2026-11-07; 49,250 came from dividing the rounded 394,000). Stale 49,250
references in `scripts/gen_xcvu3p.py` and the xcvu3p.json
`num_slices` note are fixed in this session (commit 98a2ce1).
Regression test pins 49,260 x 8 = 394,080 and x 16 = 788,160,
and the provisional geometry (821 CLB columns x 60) matching
the header. Vivado confirmation must still re-check the count.

### (c). xcku3p frame count: file 41,460 vs UG570 41,476

Status: OPEN, verified, narrowed, not closable from public
sources. The gap is 16 frames, exactly 4 per FAR row, and the
4-per-row pattern is confirmed on eight devices whose UG570
counts were checked (xcku3p, xcku5p, xcau20p, xcau25p,
xcau10p, xcau15p, xcku025, xcku035: UG570 41,476 / 41,476 /
14,376 / 14,376 / 32,530 / 32,530, file 41,460 / 41,460 /
14,364 / 14,364 / 32,510 / 32,510). The replacement-worker
check 2026-11-19 extends the pattern beyond UltraScale+:
xcku025/xcku035 are 123-word-frame UltraScale parts and show
the same 4.00 per row (gap 20 over 5 rows). The
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

Numerical cross-check, not ground truth (replacement worker,
2026-11-19): for the nine XCU SSI files, `file frames + 4 x
rows` equals a UG570 Table 1 value exactly (xcu200 215,400 +
60 = 215,460 = VU9P; xcu25 97,647 + 44 = 97,691 = KU15P;
xcu250 304,528 + 64 = 304,592 = VU13P/VU27P/VU29P; xcu26 and
xcux35 175,340 + 44 = 175,384 = KU19P/VU23P; xcu280 and
xcu55c 228,396 + 48 = 228,444 = VU11P/VU37P/VU47P/VU57P;
xcu50 and xcu55n 152,264 + 32 = 152,296 = VU35P/VU45P).
That is consistency with the 4-per-row rule, not proof of a
die equivalence: the XCU-to-UG570-device mapping is
unverified, and no UG570 row exists to check the 15 Zynq
MPSoC/RFSoC files (xczu/xazu), xck26, or xcu30 (51,882
frames, 6 rows; 51,906 matches no UG570 Table 1 value).
17 of 40 files are consistent with exactly 4 missing frames
per FAR row (8 direct, 9 numerical); 23 have no UG570
reference to check against. Do not use the XCU matches as
ground truth for any device-file value.

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

## Replacement-worker verification (session 18, 2026-11-19)

The first device-model worker errored on a runtime transport
fault (sqlx pool timeout), not on a work failure. The
replacement worker re-verified every claim above from the
primary sources instead of trusting the commits, using only
lightweight reads (python3 over the vendored JSON and the
reference clone, `git show`, `grep`, one live UG570 read).
Findings, all reproduced this session:

- eb2af3e verified: xcku3p.json header 27,120 / 162,720 /
  325,440 / 1,368 / 360; geometry 113 CLB cols x 4 rows x 60
  = 27,120; minors sums 41,460 frames; DSP 19 cols/row x 18
  = 1,368; BRAM 10 cols/row x 9 = 360; KU3P/KU5P rowMajors
  byte-identical; KU5P header 27,120 / 216,960 / 433,920 /
  1,824 / 480 (exactly 4/3 on LUTs, DSPs, BRAMs).
  `create_device_summary.py` line:
  `summary["num_slices"] = part_properties["SLICES"]`.
  All-40 header-vs-geometry check re-run: 0 mismatches (see
  the xcku025 qualification in (b1) above).
- 98a2ce1 verified: `parts_webpack.json` lists
  `Virtex UltraScale+ xcvu3p: []` and 36 xcku3p parts; all
  40 reference device files carry license Webpack; xcvu3p
  is listed in `parts_all.json` (Virtex UltraScale+, 7
  parts). The corrected license note is in the committed
  xcvu3p.json; `grep -c "appears in parts_webpack"` is 0 in
  xcvu3p.json and gen_xcvu3p.py (2 hits only in the
  regression test that asserts the phrase is absent). No
  file presents 49,250 as a live slice count or discrepancy:
  the remaining `49,250` mentions in device.rs, gen_xcvu3p.py,
  device_file_xcvu3p.rs, and xcvu3p.json are all historical
  ("the earlier 49,250 came from dividing the rounded
  394,000", "49,250 was a rounding artifact"). The `49,250`
  hits in ULTRASCALE_PLUS.json and
  ultrascale_plus_lut_ff_bram.json are (minor 49, frame_ofst
  250) bit-location pairs, unrelated to slice counts.
  `python3 scripts/gen_xcvu3p.py --check` reproduces the committed
  file: slices 49,260, frames 73,035, overshoot 1,215, 18
  provisional notes, self-checks PASS, exit 0.
- G2 verified: device.rs grid is 144 wide x 400 tall
  (lines 153-154); `tile_base_far` exists only on the
  `BitgenDevice` trait (bitgen.rs:443, implemented by
  `DeviceFile` in bitgen_device.rs:338 and the test stub),
  never on `UltraScaleDevice`; grid.rs still carries the
  provisional `CLBLL_L_X{x}Y{y}` tile names, owned by the
  integration lane. xcku3p.json SHA-256 re-computed:
  07e6a7d3d586bf9ae3ad6c5d0959b669c5332af36f473ecd44e169c47512cfbe,
  identical to the vendored copy and the reference clone.
- UG570 re-verified live from docs.amd.com 2026-11-19
  (page crawled 15:11 UTC): Table 1 KU3P 41,476 frames, 93
  words, bitstream 123,449,056 bits, overhead 515 words;
  VU3P 71,820 frames; AU20P/AU25P 41,476; KU5P 41,476;
  AU10P/AU15P 14,376; KU025/KU035 32,530. Table 2 KU3P
  X4A63093 revision 0 or later, VU3P X4B39093 revision 1
  or later. XMP103 v2.8 resource rows confirm KU3P 163K
  LUTs / 325K FFs / 1,368 DSP and VU3P 394K LUTs / 788K
  FFs / 2,280 DSP / 90.0 Mb URAM.
- Frame-gap extension (this session): the 4-per-row gap now
  holds on 8 devices with a direct UG570 Table 1 row
  (added xcau10p, xcau15p, xcku025, xcku035), including
  123-word-frame UltraScale parts, plus 9 XCU SSI files
  whose `file + 4 x rows` matches a UG570 value exactly
  (mapping unverified, recorded as numerical consistency
  only). See (c) above.
- Test status, open: the new regression tests in
  `tests/device_model_counts.rs` (6 tests) and the
  `tests/device_file_xcvu3p.rs` license-note test have NOT
  been run green on this branch in either worker session.
  The first worker's `cargo test -j2 -p helion-ultrascale
  --offline` (started 14:22, log
  /tmp/devmodel-baseline-test.log) died compiling
  `sv-parser-syntaxtree` with rustc SIGKILLed (signal 9,
  OOM) under 20+ concurrent cargo processes; it never
  reached any test. The replacement worker started no
  build: load average was 9.67 at 15:10 and 21.66 at
  15:14 with 35 cargo processes running. A single `-j1`
  targeted run (`cargo test -j1 -p helion-ultrascale
  --offline --test device_model_counts --test
  device_file_xcvu3p`) is the next step once the machine
  quiets; until it passes, the new tests are code-reviewed
  and python-verified only, not cargo-verified.

## Commits on fm/ultra-bitgen-devmodel (replacement worker, this session)

- (this file) docs(ultrascale): replacement-worker
  verification of GAP-F7, frame-gap extension to 8 direct
  devices plus 9 XCU numerical matches, DSP/BRAM grounding
  refinement, xcku025 qualification. No code changes: no
  first-worker claim was found wrong, so nothing was
  reverted or corrected.
