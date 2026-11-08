# Tilegrid research: xcku3p ground truth for IO_CONFIG (and the INT rule)

Date: 2026-11-08. Researcher: tilegrid researcher, orchestrator session 14.
Base state: branch `fm/ultra-bitstream` at fa58d5c, worktree
`~/workspace/helion-ultra-bitstream`, clean except untracked
`bitfiltrator-ref/`. LUT_INIT, FF_INIT, INT_PIP, FRAMING green tier 1 on
xcku3p for blinky and counter (critic session 15 PASS). IO_CONFIG blocked,
55 bits on both designs.
Read-only work: no commits, no code changes, no branch. No file was
modified except this report.

## Verdict up front

1. No public source contains an xcku3p tilegrid. Not BitFiltrator, not
   prjuray-db, not anywhere else in this workspace. A full xcku3p tilegrid
   cannot be grounded from public sources today.
2. An HPIO-only base FAR (one column major plus one row major) is
   necessary but NOT sufficient to turn IO_CONFIG green, even as
   absolute-provisional. Four further items are missing, and one of them
   may invalidate the golden itself (see Q4).
3. The provisional INT rule (INT column major = CLB column major - 1) is
   WRONG as a general rule. It is correct only for CLEL_R columns. For
   CLEL_L, CLEM, and CLEM_R the INT tile sits at CLB column major + 1.
   The golden's single INT column is CLEL_R, so the golden's INT_PIP
   green is unaffected, but the rule as written must be corrected before
   it is used on any other column (see Q5).
4. New flag, not in the brief: the vendored xcku3p device file lists
   tile type HPIO_L, not HPIO_RIGHT. Every non-Zynq UltraScale+ device
   file in BitFiltrator shows the same pattern; only Zynq xczu1eg/2eg/3eg
   (and xazu1eg/2eg/3eg) show HPIO_RIGHT. The golden's tile type
   HPIO_RIGHT may not exist on xcku3p at all. Only a Vivado probe can
   settle it (see Q1, Q4, Q6).
5. Recommendation: IO_CONFIG stays blocked for tier-1 green. The
   builder worker should run the narrow Vivado probe in Q6 (HPIO tile
   inventory plus one HPIO tile FAR). If the probe confirms a real
   HPIO_RIGHT tile on xcku3p, IO_CONFIG can go absolute-provisional
   (same status INT_PIP holds today) on a one-tile table. If the probe
   finds no HPIO_RIGHT, the golden needs a CTO ruling, not a builder
   fix, because there is no public HPIO_L segbits encoding anywhere.

Sources used, legal only: prjuray-db (CC0, commit affbc5e) and
BitFiltrator (MIT, commit 79b9bd6). No RapidWright .dat files and no
FPGA Interchange DBs were read or ingested.

Every claim below ships with the exact command and its output. Outputs
are pasted, not paraphrased. Where an output was long, only the
relevant lines are shown, and the trimming is stated.

---

## Q1. What does BitFiltrator xcku3p.json actually contain about tile positions?

### Q1.1 The two copies are byte-identical

Command:

```
diff /home/hatch/workspace/helion-ultra-bitstream/crates/helion-ultrascale/data/xcku3p.json /home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref/resources/devices/xcku3p.json && echo IDENTICAL || echo DIFFER
md5sum /home/hatch/workspace/helion-ultra-bitstream/crates/helion-ultrascale/data/xcku3p.json /home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref/resources/devices/xcku3p.json
```

Output:

```
IDENTICAL
e4e060cb73f243eccbe9dacf23072897  /home/hatch/workspace/helion-ultra-bitstream/crates/helion-ultrascale/data/xcku3p.json
e4e060cb73f243eccbe9dacf23072897  /home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref/resources/devices/xcku3p.json
```

The harness copy is also identical:

```
diff -q /home/hatch/workspace/ultra-bitgen-crew/harness/reference/xcku3p.json /home/hatch/workspace/helion-ultra-bitstream/crates/helion-ultrascale/data/xcku3p.json && echo SAME || echo DIFFER
SAME
```

### Q1.2 Top-level structure

Command:

```
python3 -c "
import json, pathlib
for p in ['/home/hatch/workspace/helion-ultra-bitstream/crates/helion-ultrascale/data/xcku3p.json','/home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref/resources/devices/xcku3p.json']:
    d=json.load(open(p))
    print('==',p)
    print('top keys:', list(d.keys()))
    for k,v in d.items():
        if isinstance(v, dict):
            print(f'  {k}: dict len={len(v)} keys sample={list(v.keys())[:10]}')
        elif isinstance(v, list):
            print(f'  {k}: list len={len(v)} sample={v[:3]}')
        else:
            print(f'  {k}: {type(v).__name__} = {v!r}'[:500])
"
```

Output (identical for both files, shown once):

```
== /home/hatch/workspace/helion-ultra-bitstream/crates/helion-ultrascale/data/xcku3p.json
top keys: ['device', 'license', 'num_brams', 'num_dsps', 'num_luts', 'num_regs', 'num_slices', 'num_slrs', 'part', 'slrs', 'tileType_siteType_pairs']
  device: str = 'xcku3p'
  license: str = 'Webpack'
  num_brams: int = 360
  num_dsps: int = 1368
  num_luts: int = 162720
  num_regs: int = 325440
  num_slices: int = 27120
  num_slrs: int = 1
  part: str = 'xcku3p-ffva676-1-e'
  slrs: dict len=1 keys sample=['SLR0']
  tileType_siteType_pairs: list len=65 sample=[['AMS', 'SYSMONE4'], ['BRAM', 'RAMB181'], ['BRAM', 'RAMBFIFO18']]
```

### Q1.3 slrs / SLR0 / rowMajors

Command (keys and per-table sizes for all four row majors):

```
python3 << 'PY'
import json
p='/home/hatch/workspace/helion-ultra-bitstream/crates/helion-ultrascale/data/xcku3p.json'
d=json.load(open(p))
slr=d['slrs']['SLR0']
print("SLR0 keys:", list(slr.keys()))
for k,v in slr.items():
    if isinstance(v, dict):
        print(f" {k}: dict len={len(v)} sample keys={list(v.keys())[:8]}")
    elif isinstance(v, list):
        print(f" {k}: list len={len(v)} first={v[:2]}")
    else:
        print(f" {k}: {v!r}")
PY
```

Output:

```
SLR0 keys: ['config_order_idx', 'idcode', 'max_clock_region_col_idx', 'max_clock_region_row_idx', 'max_far_row_idx', 'min_clock_region_col_idx', 'min_clock_region_row_idx', 'min_far_row_idx', 'rowMajors']
 config_order_idx: 0
 idcode: '0x04a63093'
 max_clock_region_col_idx: 3
 max_clock_region_row_idx: 3
 max_far_row_idx: 3
 min_clock_region_col_idx: 0
 min_clock_region_row_idx: 0
 min_far_row_idx: 0
 rowMajors: dict len=4 sample keys=['0', '1', '2', '3']
 slr_idx: 0
```

Each rowMajor holds exactly these 10 keys (rowMajor 0 shown; rows 1, 2,
3 are identical in CLB/BRAM/DSP tables and in the minors table, and
differ only in `min_dsp_y_ofst` / `max_dsp_y_ofst`):

```
rowMajor 0: keys=['bram_content_colMajors', 'bram_content_parity_colMajors', 'bram_reg_colMajors', 'clb_colMajors', 'clb_tileTypes', 'dsp_colMajors', 'max_dsp_y_ofst', 'min_dsp_y_ofst', 'num_minors_per_bram_content_colMajor', 'num_minors_per_std_colMajor']
  bram_content_colMajors: len=10 min=0 max=9
  bram_content_parity_colMajors: len=10 min=0 max=9
  bram_reg_colMajors: len=10 min=9 max=211
  clb_colMajors: len=113 min=5 max=226
  clb_tileTypes: len=113
  dsp_colMajors: len=19 min=14 max=222
  max_dsp_y_ofst: 23
  min_dsp_y_ofst: 0
  num_minors_per_bram_content_colMajor: [256, 256, 256, 256, 256, 256, 256, 256, 256, 256]
  num_minors_per_std_colMajor: [8, 4, 12, 4, 76, 16, 16, 76, 16, 6, ...] (len 230)
```

RowMajor 1: `min_dsp_y_ofst` 24, `max_dsp_y_ofst` 47. RowMajor 2:
48 / 71. RowMajor 3: 72 / 95. All other tables identical across rows
(verified by direct equality comparison; output: `clb identical to
row0? True minors identical? True` for rows 1, 2, 3).

So the file is a column-major index, not a tile map:

- `clb_colMajors`: logical SLICE X (0..112) to FAR column major
  (5..226), 113 entries.
- `clb_tileTypes`: logical SLICE X to tile type (CLEL_L, CLEL_R,
  CLEM, CLEM_R). Counts: CLEL_R 52, CLEM 46, CLEL_L 9, CLEM_R 6.
- `bram_content_colMajors` / `bram_content_parity_colMajors`:
  logical BRAM X (0..9) to BRAM-content column major (0..9).
- `bram_reg_colMajors`: logical BRAM X to standard column major
  (9, 30, 54, 78, 97, 121, 145, 174, 198, 211).
- `dsp_colMajors`: logical DSP X (0..18) to standard column major
  (14, 23, 35, 44, 59, 65, 71, 102, 108, 114, 126, 135, 150, 164,
  179, 185, 191, 216, 222).
- `num_minors_per_std_colMajor`: length 230, indexed by FAR column
  major 0..229. Value set: {4, 6, 8, 9, 10, 12, 16, 76}.
  Histogram: 16 appears 113 times, 76 appears 75 times, 8 appears
  21 times, 6 appears 10 times, 4 appears 7 times, 10 appears twice,
  12 once, 9 once.

### Q1.4 No table maps a tile name to a FAR column / row major

Command:

```
python3 << 'PY'
import json
p='/home/hatch/workspace/helion-ultra-bitstream/crates/helion-ultrascale/data/xcku3p.json'
txt=open(p).read()
print("contains HPIO_RIGHT?", "HPIO_RIGHT" in txt)
print("contains HPIO?", "HPIO" in txt)
print("contains INT_X?", "INT_X" in txt)
print("contains SLICE_X?", "SLICE_X" in txt)
print("contains colMajors keys count:", txt.count("colMajors"))
PY
```

Output:

```
contains HPIO_RIGHT? False
contains HPIO? True
contains INT_X? False
contains SLICE_X? False
contains colMajors keys count: 20
```

The only HPIO mentions are the seven `HPIO_L` pairs in
`tileType_siteType_pairs` (lines 109-115 of the file):

```
["HPIO_L","BIAS"], ["HPIO_L","HPIO_VREF_SITE"], ["HPIO_L","HPIOB_M"],
["HPIO_L","HPIOB_S"], ["HPIO_L","HPIOB_SNGL"],
["HPIO_L","HPIOBDIFFINBUF"], ["HPIO_L","HPIOBDIFFOUTBUF"]
```

There is no `HPIO_RIGHT` pair, no `CMT_RIGHT` pair, no tile name
anywhere in the file, no grid_x / grid_y, no clock-region tile list,
no HPIO/HDIO/CMT column table. `tileType_siteType_pairs` is a
tile-type to site-type inventory only: it says which site types occur
in which tile types somewhere on the device. It does not say where any
tile is, how many such tiles exist, or which FAR column any of them
occupies.

The raw input that BitFiltrator's `create_device_summary.py`
consumed (the Vivado `extract_device_info.tcl` output, with
`composition / clock_regions / tile_cols` mapping every tile name to
its sites) is NOT vendored. Only this summary survived. The tile
inventory was discarded in summarization, except for the SLICE
derivation in `clb_tileTypes`.

### Q1.5 HPIO_L vs HPIO_RIGHT across all BitFiltrator device files

Command:

```
python3 -c "
import json, glob
for p in sorted(glob.glob('/home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref/resources/devices/*.json')):
    d=json.load(open(p))
    pairs=[t for t,s in d['tileType_siteType_pairs']]
    print(p.split('/')[-1], 'HPIO_RIGHT' in pairs, 'HPIO_L' in pairs, 'HDIO_BOT_RIGHT' in pairs, 'CMT_RIGHT' in pairs, 'CMT_L' in pairs)
"
```

Output (columns: file, HPIO_RIGHT, HPIO_L, HDIO_BOT_RIGHT, CMT_RIGHT, CMT_L):

```
xazu1eg.json True False True True False
xazu2eg.json True False True True False
xazu3eg.json True False True True False
xazu4ev.json False True True False True
xazu5ev.json False True True False True
xazu7ev.json False True True False True
xcau10p.json False True True False True
xcau15p.json False True True False True
xcau20p.json False True True False True
xcau25p.json False True True False True
xck26.json False True True False True
xcku025.json False True False False False
xcku035.json False True False False False
xcku3p.json False True True False True
xcku5p.json False True True False True
xcu200.json False True False False True
xcu25.json False True True False True
xcu250.json False True False False True
xcu26.json False True True False True
xcu280.json False True False False True
xcu30.json False True True False True
xcu50.json False True False False True
xcu55c.json False True False False True
xcu55n.json False True False False True
xcux35.json False True True False True
xczu1cg.json True False True True False
xczu1eg.json True False True True False
xczu2cg.json True False True True False
xczu2eg.json True False True True False
xczu3cg.json True False True True False
xczu3eg.json True False True True False
xczu4cg.json False True True False True
xczu4eg.json False True True False True
xczu4ev.json False True True False True
xczu5cg.json False True True False True
xczu5eg.json False True True False True
xczu5ev.json False True True False True
xczu7cg.json False True True False True
xczu7eg.json False True True False True
xczu7eg.json False True True False True
```

Pattern: only Zynq xczu1/2/3eg/cg and xazu1/2/3eg carry HPIO_RIGHT
(and CMT_RIGHT). Every Kintex, Artix, Virtex, and larger Zynq part
carries HPIO_L (and CMT_L). xcku3p is in the HPIO_L group. This is
consistent with prjuray-db covering Zynq US+ only, but it is NOT
proof on its own: the pairs list is BitFiltrator-generated, and its
own generation provenance (Vivado version, part string) is not
recorded in the file. The Vivado probe in Q6 is the arbiter. What it
does prove is that no public file in this workspace supports the
golden's tile type HPIO_RIGHT on xcku3p, while the one device file
that does describe xcku3p points the other way.

Corollary: prjuray-db ships `segbits_hpio_right.db` only. There is
no `segbits_hpio_left.db` in the repo (full `.db` listing in Q3
confirms this). If xcku3p has no HPIO_RIGHT tiles, the IO encoding
for the real xcku3p IO tiles is entirely missing from every public
source, and a tilegrid alone would not fix IO_CONFIG: a new HPIO_L
fuzzing campaign would be needed.

### Q1.6 Can the HPIO column be read off the minors table?

No. Columns with 8 minors (the HPIO frame count on xczu3eg, see Q3)
in xcku3p row 0:

```
cols with 8 minors: [0, 14, 23, 35, 44, 59, 65, 71, 102, 108, 114, 126, 135, 150, 164, 179, 185, 191, 216, 222, 229]
```

21 candidates. On xczu3eg, the 8-frame columns in `part.json` row 0
are [11, 42, 57, 82, 88, 103], and only col 103 is HPIO; the other
five are RCLK_DSP_INTF tiles (verified from the tilegrid, Q3). Frame
count alone cannot identify an HPIO column. The rightmost 8-minor
columns beyond the last CLB column (226) are 229 only (216 and 222
are DSP columns per `dsp_colMajors`: DSP X17 = 216, X18 = 222).
Column 229 is a candidate for the rightmost IO column by analogy
with xczu3eg, where HPIO col 103 is the last CLB_IO_CLK column
(0..103), but that is an analogy, not ground truth: 229 could equally
be an RCLK or other 8-frame tile. Do not use 229 as ground truth.

---

## Q2. How does BitFiltrator demo.py plus bit_locator.py reconstruct a bitstream?

### Q2.1 demo.py: only LUTs, FFs, BRAMs

File: `bitfiltrator-ref/src/demo.py` (4411 bytes). It takes a
bitstream plus `--luts`, `--ffs`, `--brams` only. There is no IO,
INT, DSP, or clock option. Argument parser, quoted verbatim:

```
  parser.add_argument("bitstream", type=str, help="Input bitstream (with header). Must be a full bitstream, not a partial one.")
  parser.add_argument("--luts", type=str, nargs="*", default=[], help="Name of LUTs to locate in the form of SLICE_X(\\d+)Y(\\d+)/[A-H]6LUT")
  parser.add_argument("--ffs", type=str, nargs="*", default=[], help="Name of Flip-Flops to locate in the form of SLICE_X(\\d+)Y(\\d+)/[A-H]FF2?")
  parser.add_argument("--brams", type=str, nargs="*", default=[], help="Name of 18K BRAMs to locate in the form of RAMB18_X(\\d+)Y(\\d+)")
```

Flow: `Bitstream.from_file_path` parses the bitstream;
`BitLocator(bitstream.header.fpga_part)` is constructed;
`resources.get_device_summary(bitstream.header.fpga_part)` loads
`resources/devices/<device>.json`; `extract_frames` keys frames by
(SLR name, FAR) using the SLR idcode; `print_lut_info` /
`print_ff_info` / `print_bram_info` call `bitlocator.locate_lut` /
`locate_reg` / `locate_bram` and print SLR, FAR, frame offset, and
byte offset in the bitstream. No HPIO/HDIO/INT placement exists in
this path at all.

### Q2.2 bit_locator.py: only CLB and BRAM_CONTENT block types

File: `bitfiltrator-ref/src/bit_locator.py` (8259 bytes). The class
`BitLocator` loads three things in `__init__`:

```
    self._arch_summary = resources.get_arch_summary(fpga_part)
    self._device_summary = resources.get_device_summary(fpga_part)
    self._ar_spec = ArchSpec.create_spec(fpga_part)
```

`resources.py` resolves those to `resources/archs/ULTRASCALE_PLUS.json`
and `resources/devices/<device>.json` via `parts_all.json`
(part to device and arch). The locator has exactly three public
methods: `locate_reg`, `locate_lut`, `locate_bram`. All three use the
same helpers: `_get_slr_name` (from clock-region row ranges and
60 CLBs / 24 BRAM18s per column), `_get_row_major` (relative row
major inside the SLR), and `_get_col_major`. The column lookup,
quoted verbatim:

```
  def _get_col_major(
    self,
    bel_x: int,
    slr_name: str,
    row_major: int,
    block_type: FarBlockType
  ) -> tuple[int, str]:
    assert block_type in {FarBlockType.CLB_IO_CLK, FarBlockType.BRAM_CONTENT}, f"Error: Cannot generate col major for unknown block type {block_type}"

    if block_type == FarBlockType.CLB_IO_CLK:
      # The x-value is the CLB column number. We lookup the device resource file to map this
      # logical CLB column number to a physical major column number.
      colMajor = self._device_summary.get_clb_col_majors(slr_name, row_major)[bel_x]
      tileType = self._device_summary.get_clb_tile_types(slr_name, row_major)[bel_x]
    else:
      colMajor = self._device_summary.get_bram_content_col_majors(slr_name, row_major)[bel_x]
      # All BRAM columns have the same tile type.
      tileType = "BRAM"
```

The per-BEL encodings come from `ArchSummary.get_lut_loc` /
`get_reg_loc` / `get_bram_mem_loc` / `get_bram_parity_loc`, which read
`ULTRASCALE_PLUS.json`. That file has exactly five top-level keys:

```
ULTRASCALE_PLUS.json top keys: ['BRAM', 'CLEL_L', 'CLEL_R', 'CLEM', 'CLEM_R']
BRAM dict len 2 sample ['BramMemLoc', 'BramMemParityLoc']
CLEL_L dict len 2 sample ['LutLoc', 'RegLoc']
CLEL_R dict len 2 sample ['LutLoc', 'RegLoc']
CLEM dict len 2 sample ['LutLoc', 'RegLoc']
CLEM_R dict len 2 sample ['LutLoc', 'RegLoc']
```

No HPIO, HDIO, INT, DSP, CMT, or clock section exists in the arch
file. Command confirming no HPIO/HDIO string exists anywhere in the
BitFiltrator Python sources or arch JSONs:

```
grep -rn "HPIO\|HDIO\|hpio\|hdio" /home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref/src/*.py /home/hatch/workspace/helion-ultra-bitstream/bitfiltrator-ref/resources/archs/*.json
```

Output: empty (no matches). The `device_summary.py` getters are
likewise only `get_bram_content_col_majors`,
`get_bram_parity_col_majors`, `get_bram_reg_col_majors`,
`get_dsp_col_majors`, `get_clb_col_majors`, `get_clb_tile_types`,
plus the minors tables. There is no HPIO/HDIO/INT getter.

### Q2.3 Do those tables cover xcku3p at all?

The device file exists for xcku3p (`resources/devices/xcku3p.json`,
part `xcku3p-ffva676-1-e`), and the arch file is per-architecture
(UltraScale+), not per-device, so LUT/FF/BRAM location works for
xcku3p. That is exactly the coverage the harness already uses, and
it is why LUT_INIT / FF_INIT are green. But HPIO/HDIO placement is
not covered for xcku3p or for any other device: no table, no code
path, no demo option exists for it in BitFiltrator.

### Q2.4 Why the device file has no IO columns: the generator never extracts them

`create_device_summary.py` builds the device file from Vivado runs.
Its steps, in order: `extract_device_info.tcl` (PinPlanning probe,
tile inventory and site pairs), empty bitstream, per-frame-CRC
bitstream, FAR enumeration, SLR idcodes, num majors/minors, one FF
in every CLB column (.ll file, `extract_col_majors_from_ll` with
filter `RegLoc`), one BRAM in every BRAM column (.ll file, filter
`Bram.*`), CLB tile types from the device info, then one DSP in
every DSP column (bit diff, `extract_dsp_col_majors_from_bit`).
There is no step that places an IOB in an HPIO/HDIO column, no step
that extracts an IO column major, and no IO table in the summary
schema. Re-running BitFiltrator's own pipeline as-is would still
produce no HPIO column. A new step (new Tcl placer plus new
extractor, modeled on the CLB/BRAM/DSP steps) would be required.
The `extract_device_info.tcl` probe itself is the reusable part: it
already enumerates every tile by name per clock-region column with
its sites, via:

```
vivado -mode batch -source extract_device_info.tcl -notrace -tclargs <fpga_part> <json_out>
```

with `create_project ... -in_memory -part ${fpga_part}`,
`set_property DESIGN_MODE PinPlanning`, and `open_io_design`. That
probe answers tile existence and tile names (Q6 step 1) but not FAR
column majors; those need the bitstream/.ll step.

---

## Q3. What does the xczu3eg tilegrid.json in prjuray-db contain?

### Q3.1 Files and identity

```
ls /home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/ /home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sfvc784-1-e/
```

Output:

```
/home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/:
package_pins.csv
part.json
part.yaml
tileconn.json
tilegrid.json

/home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sfvc784-1-e/:
package_pins.csv
part.json
part.yaml
tileconn.json
tilegrid.json
```

The two tilegrids are byte-identical (same part, different packages;
the tilegrid is package-independent):

```
diff -q /home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tilegrid.json /home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sfvc784-1-e/tilegrid.json && echo TILEGRID_IDENTICAL || echo TILEGRID_DIFFER
TILEGRID_IDENTICAL
md5sum /home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tilegrid.json /home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sfvc784-1-e/tilegrid.json
7cea605749f946f896338fa527fccf1e  /home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tilegrid.json
7cea605749f946f896338fa527fccf1e  /home/hatch/workspace/prjuray-db-ref/zynqusp/xczu3eg-sfvc784-1-e/tilegrid.json
```

Size: 15 MB, 66,385 tiles, 158 distinct tile types. `part.json`
idcode 77865107 (0x4a42093), 3 rows (clock-region rows 0, 1, 2), each
with BLOCK_RAM (6 columns, 256 frames each) and CLB_IO_CLK (104
columns).

### Q3.2 Schema

Top level: a dict keyed by Vivado tile name
(`<TYPE>_X<x>Y<y>`) to a tile record. Fields, from direct reads:

- `type`: tile type string, e.g. `HPIO_RIGHT`, `INT`, `CLEL_R`.
- `grid_x`, `grid_y`: integer grid coordinates (physical floorplan
  position; note grid_y runs opposite to tile Y: tile Y0 has
  grid_y 185, tile Y150 has grid_y 30).
- `bits`: dict keyed by configuration bus (`CLB_IO_CLK`,
  `BLOCK_RAM`), each value `{baseaddr, frames, offset, words}`.
  `baseaddr` is a hex string holding a packed FAR (see Q3.4).
  Empty `{}` for tiles with no configuration bits (e.g. RBRK and
  TERM tiles, NULL tiles). 15,496 of 66,385 tiles have non-empty
  `bits`.
- `sites`: dict of site name to site type, e.g.
  `{"SLICE_X7Y0": "SLICEL"}` for CLB tiles,
  `{"IOB_X2Y0": "HPIOB_M", ...}` for HPIO tiles. Empty `{}` for INT
  tiles (INT has no sites).
- `pin_functions`: dict of site name to `{package_pin: function}`,
  populated for IO tiles (HPIO, HDIO), empty `{}` otherwise.
- `clock_region`: e.g. `"X1Y0"`, present on HPIO, HDIO, and CLB
  tiles, absent on INT tiles.

Example entries (pasted from the file, `pin_functions` truncated
where marked):

HPIO:

```
HPIO_RIGHT_X32Y0 {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00006700", "frames": 8, "offset": 5, "words": 85}}, "clock_region": "X1Y0", "grid_x": 309, "grid_y": 185, "pin_functions": {"IOB_X2Y0": {"AC9": "IO_L1P_T0L_N0_DBC_64"}, "IOB_X2Y1": {"AD9": "IO_L1N_T0L_N1_DBC_64"}, ...}, "sites": {"BIAS_X0Y0": "BIAS", "HPIOBDIFFINBUF_X0Y0": "HPIOBDIFFINBUF", ..., "IOB_X2Y0": "HPIOB_M", "IOB_X2Y1": "HPIOB_S", ..., "IOB_X2Y25": "HPIOB_SNGL"}, "type": "HPIO_RIGHT"}
```

HDIO:

```
HDIO_BOT_RIGHT_X9Y0 {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00001E00", "frames": 10, "offset": 0, "words": 87}}, "clock_region": "X0Y0", "grid_x": 200, "grid_y": 185, "pin_functions": {"IOB_X0Y0": {"AE15": "IO_L1P_AD15P_24"}, ...}, "sites": {...}, "type": "HDIO_BOT_RIGHT"}
```

INT:

```
INT_X0Y0 {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00000100", "frames": 76, "offset": 0, "words": 3}}, "grid_x": 160, "grid_y": 185, "pin_functions": {}, "sites": {}, "type": "INT"}
```

CLB:

```
CLEL_L_X5Y0 {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00000F00", "frames": 16, "offset": 0, "words": 3}}, "clock_region": "X0Y0", "grid_x": 180, "grid_y": 185, "pin_functions": {}, "sites": {"SLICE_X7Y0": "SLICEL"}, "type": "CLEL_L"}
CLEM_X1Y0 {"bits": {"CLB_IO_CLK": {"baseaddr": "0x00000300", "frames": 16, "offset": 0, "words": 3}}, "clock_region": "X0Y0", "grid_x": 164, "grid_y": 185, "pin_functions": {}, "sites": {"SLICE_X1Y0": "SLICEM"}, "type": "CLEM"}
```

Note the SLICE_X vs tile-X split that BitFiltrator also documents:
`CLEL_L_X5Y0` holds `SLICE_X7Y0`. The site name, not the tile name,
gives the logical CLB column. The same split applies to IOB sites:
the HPIO tile `HPIO_RIGHT_X32Y0` holds IOB sites `IOB_X2Y0`..`Y25`
(global IOB column 2), while the prjuray segbits DB names the same
IOBs `IOB_X0Y0`..`Y25` (tile-relative index, X always 0; verified:
distinct IOB X values in `segbits_hpio_right.db` are `['0']`, 26
IOB names total). The golden's `IOB_X0Y0` / `IOB_X0Y1` follow the
segbits-relative convention, not Vivado global site names.

### Q3.3 Tile counts for the types that matter

From the sbva484 tilegrid (type counts, selected):

```
BRAM: 216, CLEL_L: 720, CLEL_R: 4500, CLEM: 2520, CLEM_R: 1080,
DSP: 180, HDIO_BOT_RIGHT: 4, HDIO_TOP_RIGHT: 4,
HPIO_RIGHT: 6, INT: 5940
```

HPIO_RIGHT tiles, all six:

```
HPIO_RIGHT_X32Y0, HPIO_RIGHT_X32Y30, HPIO_RIGHT_X32Y60,
HPIO_RIGHT_X32Y90, HPIO_RIGHT_X32Y120, HPIO_RIGHT_X32Y150
```

Two per clock-region row. All six share column major 103; rows 0, 0,
1, 1, 2, 2; `bits.offset` 5 for Y0/Y60/Y120 and 101 for
Y30/Y90/Y150; `frames` 8 each. Two distinct tiles share one
(block, row, col) triple and one baseaddr, and differ in `offset`
and in IOB range (Y0 tile: IOB_X2Y0..Y25; Y30 tile: IOB_X2Y26..).
A (row, col) pair alone therefore does NOT uniquely identify an
HPIO tile's bits. This matters for Q4.

HDIO tiles: `HDIO_BOT_RIGHT_X9Y0` col 30 row 0 frames 10,
`HDIO_BOT_RIGHT_X24Y0` col 76 row 0, plus TOP variants at the same
columns. INT tiles: 5,940, `frames` 76 each.

### Q3.4 baseaddr is a packed FAR

UltraScale+ FAR layout (UG570 table 9-21, via BitFiltrator
`arch_spec.py` `UltraScalePlusSpec`): bits 31:27 reserved, 26:24
block type, 23:18 row, 17:8 column, 7:0 minor. Decoding the
tilegrid baseaddrs with that layout:

```
INT_X11Y60 0x00042300 -> block 0, row 1, col 35, minor 0
CLEL_R_X11Y60 0x00042400 -> block 0, row 1, col 36, minor 0
INT_X0Y0 0x00000100 -> block 0, row 0, col 1, minor 0
CLEL_R_X0Y0 0x00000200 -> block 0, row 0, col 2, minor 0
HPIO_RIGHT_X32Y0 0x00006700 -> block 0, row 0, col 103, minor 0
```

Every HPIO/INT/CLB baseaddr minor observed is 0, so the segbits
frame number equals the FAR minor directly, for these tile types, on
xczu3eg. That is the property the harness relies on when it adds a
segbits frame to a tile base FAR.

Cross-check against `part.json`: row 0 CLB_IO_CLK has 104 columns;
frame-count histogram: 16 frames in 50 columns, 76 frames in 33
columns, 8 frames in 6 columns [11, 42, 57, 82, 88, 103], 10 frames
in 2 columns, 6 in 6, 4 in 6, 12 in 1. The 76-frame columns match
the INT columns; the 16-frame columns match CLB columns; the
8-frame columns are five RCLK_DSP_INTF columns plus the HPIO
column 103 (verified tile by tile from the tilegrid).

### Q3.5 Could the same schema be built for xcku3p from any public source?

No. The prjuray-db repo contains only `zynqusp` (one directory;
verified by `ls`: CONTRIBUTING.md, COPYING, Info.md, Makefile,
README.md, zynqusp). It covers xczu3eg only (two packages, same
tilegrid). Its README states the DB was generated with the Project
U-Ray tools (SymbiFlow/prjuray) and that a prebuilt DB is provided
because generating it takes a long time. There is no Kintex,
Artix, or Virtex part in the repo, and no xcku3p tilegrid, part
file, or tileconn file.

The same schema for xcku3p could be built only from Vivado, by two
halves:

(a) Tile inventory half, from Vivado alone, no fuzzing: run
BitFiltrator's `extract_device_info.tcl` (PinPlanning probe) for
`xcku3p-ffva676-1-e` and keep the raw output. That gives every tile
name, its Vivado column/row, its clock region, and its sites
(including IOB sites and package pins via `package_pins.csv`
equivalents). This half is fully scripted already.

(b) Frame-address half, from Vivado plus bitstream observation:
the `bits.baseaddr / frames / offset / words` per tile. On
prjuray-db this came from the Project U-Ray fuzzers (place one
feature per tile, build a bitstream, diff frames). The equivalent
inside BitFiltrator's own pipeline is the `.ll` / bit-diff route
used for CLB, BRAM, and DSP columns (see Q2.4), extended with a
new IO step: place one IOB in every HPIO/HDIO tile, generate the
logic-location file or bitstream, read the FARs. Either route needs
a licensed Vivado run against the xcku3p part. Nothing public
substitutes for it.

Note: the `tile_types/` directory in prjuray-db (e.g.
`tile_type_HPIO_RIGHT.json`, `tile_type_INT.json`) is per-tile-type
pip/site inventory, not a tilegrid; it carries no tile positions
and no FARs, so it does not unblock anything on its own.

---

## Q4. Minimal grounded scope that unblocks IO_CONFIG green on the v1 goldens

### Q4.1 What the harness does today

`harness/reference.py`, `resolve_hpio` (lines 332-345):

```
    def resolve_hpio(self, tile, feature):
        """Reference bits for one HPIO feature, tile-relative.

        Absolute FARs need the HPIO tile's column major, which is
        unknown for xcku3p/xcvu3p (no tilegrid). Returns
        {(tile, frame, offset): value}.
        """
        db = self.segbits("hpio_right")
        key = "HPIO_RIGHT." + feature
        if key not in db:
            raise MissingReference(
                "HPIO feature %r not in prjuray-db segbits_hpio_right.db; "
                "encoding of this IO attribute is unverified" % feature
            )
        return {(tile, frame, bit): val for frame, bit, val in db[key]}
```

`harness/diff_engine.py` lines 95-98:

```
        if block == "IO_CONFIG":
            # Feature string after the tile: e.g. IOB_X0Y0.IOSTANDARD_IN.LVCMOS12
            bits = ref.resolve_hpio(tile, feat)
            return block, bits, "tile-relative", None
```

and lines 276-285, the block that keeps IO_CONFIG from ever being
compared:

```
        # Tile-relative reference bits cannot be compared against the
        # emitter's absolute bitset until the tilegrid lands.
        if rec["coord_system"] == "tile-relative":
            rec["diff_bits"] = None
            rec["status"] = "blocked"
            rec["missing"] = (
                "reference bits are tile-relative; absolute FARs need the "
                "%s tilegrid (prjuray-db ships xczu3eg tilegrids only)" % part
            )
            records.append(rec)
            continue
```

The INT rule lives at `reference.py` lines 40-52 (comment plus
`INT_COL_OFFSET_FROM_CLB = -1`); the HPIO function takes no `part`
argument and performs no FAR computation at all, so the builder
must add a tile-to-FAR lookup on both sides: `resolve_hpio` in the
harness and `tile_base_far` in the emitter (`bitgen_device.rs`
returns `None` for HPIO today; the binary keeps the bridge, see
Q4.4).

### Q4.2 The 55 bits, verified

Command:

```
python3 << 'PY'
import sys
sys.path.insert(0, '/home/hatch/workspace/ultra-bitgen-crew/harness')
from reference import Tier1Reference
ref = Tier1Reference()
for feat in ['IOB_X0Y0.IOSTANDARD_IN.LVCMOS12','IOB_X0Y1.IOSTANDARD_OUT.LVCMOS12_IDRIVE_I2_SLEW_SLEW_SLOW']:
    bits = ref.resolve_hpio('HPIO_RIGHT_X0Y0', feat)
    print(feat, "->", len(bits), "tile-relative bits")
    ones = sum(1 for v in bits.values() if v==1)
    print("   ones:", ones, "zeros:", len(bits)-ones, "frames used:", sorted(set(f for (_,f,_) in bits)))
PY
```

Output:

```
IOB_X0Y0.IOSTANDARD_IN.LVCMOS12 -> 21 tile-relative bits
   ones: 4 zeros: 17 frames used: [2, 3]
IOB_X0Y1.IOSTANDARD_OUT.LVCMOS12_IDRIVE_I2_SLEW_SLEW_SLOW -> 34 tile-relative bits
   ones: 24 zeros: 10 frames used: [2, 3]
```

21 + 34 = 55, matching the critic15 `diff.jsonl` records for both
designs (`"block": "IO_CONFIG", "total_bits": 55, "status":
"blocked"`). Both golden lines sit on one tile,
`HPIO_RIGHT_X0Y0`, and use only segbits frames 2 and 3. The raw
segbits lines (from `segbits_hpio_right.db`):

```
HPIO_RIGHT.IOB_X0Y0.IOSTANDARD_IN.LVCMOS12 !02_46 !02_47 !02_49 !02_50 !02_61 !02_65 02_73 02_74 02_109 !02_110 !02_111 !03_45 !03_46 !03_49 !03_60 !03_65 03_73 !03_86 !03_109 !03_110 !03_111
HPIO_RIGHT.IOB_X0Y1.IOSTANDARD_OUT.LVCMOS12_IDRIVE_I2_SLEW_SLEW_SLOW !02_113 02_123 02_124 02_125 02_126 02_127 02_129 02_130 02_132 02_133 02_134 02_201 !02_202 !02_203 03_112 !03_117 !03_118 !03_120 !03_121 03_122 03_123 03_124 03_125 03_126 03_128 03_129 03_130 03_132 03_133 03_158 !03_201 03_202 !03_203 !03_204
```

(Note: the second line as printed by the earlier grep is the
`IOB_X0Y1` OUT line; the golden's feature is
`IOB_X0Y1.IOSTANDARD_OUT.LVCMOS12_IDRIVE_I2_SLEW_SLEW_SLOW`, 34
bits, frames 2 and 3.)

### Q4.3 Is the HPIO base FAR (column major + row major) sufficient?

No. It is one of five items, and the other four are each capable of
blocking or invalidating the result:

1. Column major and row major of the real tile (the base FAR).
   Missing. Candidates from the minors table are unusable (Q1.6).
   On xczu3eg the answer for the analogous tile is col 103, row 0,
   baseaddr 0x00006700, minor 0.
2. Real tile identity. The golden's tile name `HPIO_RIGHT_X0Y0` is
   a placeholder: the golden header itself says INT and HPIO tile
   coordinates use the logical CLB column index as the tile X, NOT
   a verified Vivado tile X. Real HPIO tiles are named like
   `HPIO_RIGHT_X32Y0` on xczu3eg. On xcku3p the tile X, the tile
   count per row, and even the tile type are unknown (Q1.5). The
   harness lookup must be keyed by the real Vivado tile name, or
   the golden must be corrected to it by CTO ruling. A table keyed
   by the placeholder name `HPIO_RIGHT_X0Y0` would be a convention,
   not ground truth, and must be labeled as such.
3. Base minor 0 and frame-to-minor identity. On xczu3eg every HPIO
   baseaddr minor is 0, so segbits frame = FAR minor. This needs
   one confirmation for xcku3p; it cannot be assumed, because it is
   a property of the tile's frame allocation, not of the segbits DB.
4. The `bits.offset` rule (two tiles per clock-region row). On
   xczu3eg, the two HPIO tiles in a row share (block, row, col)
   and baseaddr, and differ in `offset` (5 vs 101) and IOB range.
   The golden uses IOB_X0Y0/Y1, the first two IOBs of a tile, so
   the builder must establish which tile in row 0 those IOBs
   belong to and how `offset` enters the absolute bit position
   (or prove it does not enter it, because the segbits bit numbers
   already encode the per-IOB shift: IOB_X0Y1 bits sit exactly 96
   above IOB_X0Y0 bits, and 101 - 5 = 96). This is an open
   question, not a settled mapping. The experiment that decides
   it: place IOBs in the second HPIO tile of a row and compare
   observed bit positions against segbits bits plus the tile
   offset.
5. Tile-type validity (Q1.5). If the Vivado probe finds no
   HPIO_RIGHT tile on xcku3p, items 1-4 are moot: the golden's
   tile type is wrong, `segbits_hpio_right.db` does not apply to
   the real IO tiles (HPIO_L), and no public HPIO_L encoding
   exists. That is a CTO decision (change the golden to the real
   tile type and fuzz HPIO_L, or move the v1 goldens to a part
   that has HPIO_RIGHT), not a builder task.

Minimal grounded scope, assuming the probe confirms HPIO_RIGHT
exists on xcku3p: exactly one tile row, namely the real Vivado
tile name for the row-0 HPIO tile holding the golden's IOBs, with
its (block, row, col) base FAR, its base minor, its frame count,
and the offset rule for item 4. That is enough because both golden
lines share one tile and only frames 2 and 3. No other tile is
needed for the v1 goldens. The result would be absolute-provisional
(the INT_PIP status today), not full tier-1 ground truth, until a
full tilegrid exists. A full xcku3p tilegrid is NOT required to
unblock these 55 bits; a one-tile probe plus the offset ruling is.

### Q4.4 The invented bridge that the tilegrid kills

The bridge is in `crates/helion-ultrascale/src/bin/helion-bitgen.rs`,
struct `HpioProvisionalFallback`, doc comment lines 207-216:

```
/// One provisional bridge stays (critic session 13 FLAG F1): the
/// real impl returns `None` for HPIO tiles, but the v1 goldens carry
/// `HPIO_RIGHT_X0Y0` lines, so that one tile still resolves to the
/// stub's provisional base FAR 0x000 with tile type `HPIO_RIGHT`.
/// The value is admitted invented (no public HPIO column major
/// exists); the harness blocks IO_CONFIG before comparing, so no
/// block's greenness depends on it. The bridge is exactly one tile,
/// not a table: every other tile the file does not resolve still
/// fails loud. It dies when the xcku3p tilegrid lands.
```

Code, lines 240-248 and 272:

```
    fn tile_type(&self, tile: &str) -> Option<&str> {
        if tile == "HPIO_RIGHT_X0Y0" {
            return Some("HPIO_RIGHT");
        }
        <DeviceFile as BitgenDevice>::tile_type(&self.inner, tile)
    }

    fn tile_base_far(&self, tile: &str) -> Option<u32> {
        if tile == "HPIO_RIGHT_X0Y0" {
            return Some(0x000);
        }
        <DeviceFile as BitgenDevice>::tile_base_far(&self.inner, tile)
    }
```

plus `names.push("HPIO_RIGHT_X0Y0".to_string())` in `load_device`.
Tests at lines 710-711 and 717 pin the invented values. The bridge
emits HPIO bits at FARs 0x00000002 and 0x00000003 (base 0x000 plus
frames 2 and 3). The real `BitgenDevice` impl in `bitgen_device.rs`
returns `None` for HPIO tiles and fails loud (`unknown tile
HPIO_RIGHT_X0Y0`, exit 1) without the bridge. When the Q4.3 probe
lands, the builder deletes `HpioProvisionalFallback` entirely and
the real impl resolves the tile from the probe table; the bridge
must not be extended to a second tile or a guessed column.

---

## Q5. Can the provisional INT rule be confirmed or corrected?

It can be corrected, from the xczu3eg tilegrid, and the correction
is sharp. No public source confirms the rule as written.

### Q5.1 The rule as written

`harness/reference.py` lines 40-52:

```
# INT column rule, derived from the xczu3eg tilegrid (prjuray-db):
# an INT tile serving a CLB column sits in the physical column major
# one below the CLB column major (e.g. INT col 35 serves CLB col 36,
# INT_X11Y60 baseaddr 0x00042300 vs CLEL_R_X11Y60 0x00042400).
# PROVISIONAL for xcku3p/xcvu3p: no xcku3p/xcvu3p tilegrid exists in any
# public DB. The device-file worker owns confirming it.
INT_COL_OFFSET_FROM_CLB = -1
```

The example is real (verified in Q3.4: INT_X11Y60 col 35,
CLEL_R_X11Y60 col 36). But it is one tile type's example presented
as a general rule.

### Q5.2 The tilegrid says the offset is type-dependent

Command (for every CLB tile with bits on xczu3eg, find the INT tile
at the same Y on its left and right grid positions, and compare FAR
column majors):

Result, CLB neighbour (left type, right type) counts:

```
  ('INT', 'NULL') 3474
  ('NULL', 'INT') 3330
  ('CLEL_R', 'INT') 720
  ('INT', 'CLEL_L') 720
  ('INT', 'BRAM') 216
  ('DSP', 'INT') 180
  ('INT', 'CFRM_CBRK_L') 66
  ('CFRM_CBRK_R', 'INT') 24
  ('CFG_M12BUF', 'INT') 22
  ('CFG_M12BUF_CFG_TERM_BOT_L', 'INT') 11
  ('CFG_M12BUF_CFG_TERM_TOP_L', 'INT') 11
  ...
```

INT neighbour column offset by CLB type and side:

```
  ('CLEL_L', 'R', 1) count 720 e.g. ('CLEL_L_X5Y0', 'INT_X5Y0', 15, 16)
  ('CLEL_R', 'L', -1) count 4500 e.g. ('CLEL_R_X0Y0', 'INT_X0Y0', 2, 1)
  ('CLEM', 'R', 1) count 2520 e.g. ('CLEM_X1Y0', 'INT_X1Y0', 3, 4)
  ('CLEM_R', 'R', 1) count 1080 e.g. ('CLEM_R_X26Y0', 'INT_X26Y0', 80, 81)
```

Read: for CLEL_R tiles, the serving INT tile is on the LEFT at
column major - 1 (4,500 tiles). For CLEL_L, CLEM, and CLEM_R tiles,
the serving INT tile is on the RIGHT at column major + 1 (720 +
2,520 + 1,080 = 4,320 tiles). The rule `-1` is correct for one of
four CLB tile types and wrong for the other three. Physically this
is the interleave pattern: INT columns sit between CLB column
pairs, serving the CLB column on one side or the other depending on
the pair orientation.

### Q5.3 The same split appears independently in the xcku3p device file

Command (for each of the 113 xcku3p CLB columns, check whether the
column major minus 1, or plus 1, carries at least 76 minors, the
INT frame span):

Output:

```
CLB cols where col-1 has >=76 minors: 52
  types: {'CLEL_R': 52}
  first 5: [('0', 5, 'CLEL_R', 76), ('2', 8, 'CLEL_R', 76), ('3', 11, 'CLEL_R', 76), ('6', 17, 'CLEL_R', 76), ('8', 20, 'CLEL_R', 76)]
CLB cols where col+1 has >=76 minors: 61
  types: {'CLEM_R': 6, 'CLEM': 46, 'CLEL_L': 9}
  first 5: [('1', 6, 'CLEM', 76), ('4', 12, 'CLEM', 76), ('5', 15, 'CLEM', 76), ('7', 18, 'CLEL_L', 76), ('9', 21, 'CLEM', 76)]
  overlap (both sides 76): 0 []
  neither: 0 []
```

Every one of the 113 CLB columns has exactly one INT-sized (76
minor) neighbour: 52 on the left, all CLEL_R; 61 on the right, all
CLEL_L / CLEM / CLEM_R. Zero overlap, zero neither. This is the
same 52/61 split `bitgen_device.rs` already records ("invalid for
61 of 113 CLB columns per row"), now explained: the -1 rule fails
exactly on the non-CLEL_R columns, and the +1 rule holds exactly
there. Two independent public sources (xczu3eg tilegrid geometry
and xcku3p minors table) agree on the split.

Corrected rule, grounded in both sources:

- CLB tile type CLEL_R: INT column major = CLB column major - 1.
- CLB tile type CLEL_L, CLEM, CLEM_R: INT column major = CLB
  column major + 1.

Status: corrected from xczu3eg geometry plus xcku3p minors-table
corroboration. Still provisional for xcku3p in the strict sense,
because no xcku3p tilegrid exists to confirm tile by tile, and the
+1 side has never been exercised by a golden (all golden INT lines
are INT_X0Y0 on CLEL_R column 0, where -1 holds). The golden's
INT_PIP green is unaffected by the correction: SLICE_X0Y0 is
CLEL_R on xcku3p (device file `clb_tileTypes["0"] = CLEL_R`,
`clb_colMajors["0"] = 5`, INT major 4, 76 minors), so -1 is the
correct branch for that one column. But `INT_COL_OFFSET_FROM_CLB`
must not be applied to any other column without the tile-type
branch, and `bitgen_device.rs` `int_col_major` (which tries -1
only and returns None otherwise) should gain the +1 branch with
the same 76-minor validation before any non-CLEL_R INT tile is
resolved. That change belongs to the builder, with a test that
resolves an INT tile on a CLEM column and checks its 76-minor
column.

---

## Q6. Exact missing data, and what to ask Vivado for

### Q6.1 What is missing

For a full xcku3p tilegrid in prjuray-db schema (tile name to
`{bits: {bus: {baseaddr, frames, offset, words}}, grid_x, grid_y,
pin_functions, sites, type, clock_region}`), for every tile on
xcku3p-ffva676-1-e:

- Every tile name and tile type, with grid position and clock
  region. Partially recoverable without fuzzing from the
  PinPlanning probe (Q2.4), but not vendored anywhere today.
- Every tile's `bits` entry: baseaddr (packed FAR), frames,
  offset, words, per bus. This is the fuzzing-derived half. No
  public source has any of it for xcku3p.
- The HPIO/HDIO/CMT/DSP column-major tables that the BitFiltrator
  device-file schema would need (`hpio_colMajors`,
  `hdio_colMajors`, etc. do not exist in the schema at all).
- The `tileconn.json` equivalent (tile-to-tile connectivity) for
  any future routing use. Out of scope for IO_CONFIG, listed so
  the next worker does not mistake the tilegrid for the whole DB.

For the minimal IO_CONFIG scope (Q4.3), only one tile row is
missing:

- Real Vivado tile name and tile type of the row-0 HPIO tile
  holding the golden's IOBs.
- Its base FAR: block type, row major, column major, base minor.
- Its frame count (expected 8 by analogy with xczu3eg HPIO).
- The `offset` rule for the second tile in the row (item 4, Q4.3).
- The tile-type verdict: does any HPIO_RIGHT tile exist on
  xcku3p-ffva676-1-e at all.

### Q6.2 The Vivado ask, step by step

All steps run against part `xcku3p-ffva676-1-e`, in-memory PinPlanning
or implementation project as noted. No RapidWright, no Interchange
DBs. License server status is the orchestrator's problem; this is
the exact capture list for when a license exists.

Step 1, tile inventory (no bitstream needed, script exists):
run BitFiltrator's `extract_device_info.tcl` and KEEP the raw
output (do not summarize it away):

```
vivado -mode batch -source extract_device_info.tcl -notrace -tclargs xcku3p-ffva676-1-e xcku3p-device-info.json
```

Capture from the output: for every tile, tile name, TILE_TYPE,
COLUMN, ROW, CLOCK_REGION, and `get_sites -of_objects <tile>`
(site names and SITE_TYPEs). Arbiter queries inside the same
session, to be pasted into the run log:

```
get_tiles -filter {TILE_TYPE =~ HPIO*}
get_tiles -filter {TILE_TYPE =~ HDIO*}
get_tiles -filter {TILE_TYPE =~ CMT*}
get_tiles -filter {TILE_TYPE == HPIO_RIGHT}
get_sites -of_objects [get_tiles -filter {TILE_TYPE =~ HPIO*}]
```

This step alone settles the HPIO_RIGHT vs HPIO_L question and
gives the real HPIO tile names and IOB site names.

Step 2, FAR column majors for the HPIO tiles (bitstream/.ll
needed): extend the BitFiltrator pattern used for CLB/BRAM/DSP.
Model on `one_ff_in_every_clb_column.tcl` and
`extract_col_majors_from_ll.py`: place one IOB in every HPIO tile
(or in the single row-0 tile for the minimal scope), generate the
logic-location (.ll) file or a bitstream, and read the FAR of each
placed IOB. Record per tile: tile name, FAR block type, row major,
column major, minor, and the tile's frame count. The minimal scope
needs this for one tile only; the full grid needs it for every
HPIO/HDIO tile.

Step 3, offset semantics (same session as step 2): place IOBs in
both HPIO tiles of clock-region row 0 and compare observed bit
positions against the segbits DB bits plus the tile `offset`
values. Record whether the second tile's bits equal the first
tile's bits shifted by (offset difference), or equal the segbits
bits for the next IOB indices, or something else. This decides
item 4 of Q4.3.

Step 4, if step 1 finds no HPIO_RIGHT: stop and escalate to the
CTO. Do not guess an HPIO_L column from the minors table, do not
reuse `segbits_hpio_right.db` for HPIO_L tiles, and do not relabel
the golden's tile type without a ruling. The follow-up work in
that case is a new HPIO_L fuzzing campaign (Project U-Ray
fuzzers, same method that produced `segbits_hpio_right.db`), which
is a separate work item with its own provenance record.

Step 5, record provenance for whatever lands: Vivado version,
part string, exact Tcl files and commands, raw output files
kept (not summarized), and the same 4-line header convention the
vendored segbits DBs use (source repo, commit, license) or an
equivalent provenance note for Vivado-derived data. The raw
`device_info.json` must be kept this time; losing it is how the
tile inventory was lost once already.

### Q6.3 What does NOT unblock anything

- Re-running BitFiltrator's `create_device_summary.py` as-is: it
  has no IO step (Q2.4).
- Reading the minors table harder: 21 eight-minor columns, five of
  six such columns on xczu3eg are not HPIO (Q1.6).
- Copying the xczu3eg tilegrid numbers to xcku3p: different part,
  different column counts (xczu3eg CLB_IO_CLK has 104 columns,
  xcku3p has 230 standard columns), different clock-region rows
  (3 vs 4). The xczu3eg numbers are a schema example and a
  rule-correction source, not xcku3p data.
- The 7-series tilegrids in this workspace
  (`xilinx-dbsrc/artix7/xc7a50t/tilegrid.json`,
  `xilinx-dbsrc/zynq7/xc7z020/tilegrid.json`): different
  architecture, no UltraScale+ FAR layout. A workspace-wide search
  confirms no xcku3p tilegrid exists anywhere here:
  `find ~/workspace -iname "*tilegrid*"` returns only 7-series,
  ECP5, and prjxray reference copies.

---

## Summary tables

### Grounded (public source, command shown)

| Fact | Source |
|---|---|
| Vendored xcku3p.json is byte-identical to BitFiltrator xcku3p.json (md5 e4e060cb73f243eccbe9dacf23072897); harness copy identical too | diff / md5sum, Q1.1 |
| xcku3p.json is a column-major index only: clb 113, bram 10, dsp 19, minors len 230; no tile names, no HPIO/HDIO/CMT columns | Q1.2-Q1.4 |
| xcku3p tile pairs list HPIO_L, not HPIO_RIGHT; all non-Zynq US+ parts match, only xczu1/2/3eg/cg and xazu1/2/3eg have HPIO_RIGHT | Q1.5 |
| BitFiltrator demo/locator cover LUT/FF/BRAM only; arch file has 5 keys (BRAM, CLEL_L/R, CLEM/R); zero HPIO/HDIO mentions in src or arch JSONs | Q2 |
| xczu3eg tilegrid schema, 66,385 tiles, 158 types, sbva484 and sfvc784 byte-identical (md5 7cea605749f946f896338fa527fccf1e) | Q3.1-Q3.3 |
| baseaddr is a packed UltraScale+ FAR; HPIO col 103 row 0 minor 0 on xczu3eg; INT col = CLB col - 1 for CLEL_R, + 1 for CLEL_L/CLEM/CLEM_R | Q3.4, Q5.2 |
| IO_CONFIG is 55 bits: 21 (IOB_X0Y0 IN) + 34 (IOB_X0Y1 OUT), frames 2 and 3 only, one tile | Q4.2 |
| INT split corroborated on xcku3p minors table: 52 left (all CLEL_R), 61 right (all other CLB types), 0 overlap, 0 neither | Q5.3 |
| No xcku3p tilegrid exists anywhere in this workspace | Q6.3 |

### Provisional (flagged, do not consume as ground truth)

| Value | Status |
|---|---|
| INT rule -1, as written in `reference.py` (`INT_COL_OFFSET_FROM_CLB = -1`) | Wrong in general; correct only for CLEL_R. Corrected rule in Q5. Golden unaffected (its column is CLEL_R). |
| HPIO bridge, `helion-bitgen.rs` `HPIO_RIGHT_X0Y0` to base FAR 0x000 | Invented, admitted in code comments (critic 13 FLAG F1). Dies when the probe lands. HPIO bits currently at FARs 0x00000002 / 0x00000003. |
| HPIO column 229 as the rightmost IO column candidate | Analogy only (xczu3eg HPIO is the last CLB_IO_CLK column). 21 eight-minor candidates; do not use. |
| Golden tile name `HPIO_RIGHT_X0Y0` and IOB names `IOB_X0Y0`/`Y1` | Placeholder coordinates per the golden header. IOB names follow the segbits-relative convention (verified); the tile name and tile type are unverified. |
| xcku3p.json `xcvu3p.json` frame totals and header counts | Out of scope; noted in `bitgen_device.rs` docs, not re-verified here. |

### Missing (exact)

| Data | Provider |
|---|---|
| HPIO_RIGHT existence verdict on xcku3p-ffva676-1-e, plus real HPIO tile names, types, columns, rows, sites | Vivado PinPlanning probe, Q6 step 1 |
| One HPIO tile base FAR (block, row, col, minor) + frame count + offset rule | Vivado .ll / bitstream probe, Q6 steps 2-3 |
| Full xcku3p tilegrid (`bits` per tile for all tiles) | Vivado + Project U-Ray style fuzzing, Q6 steps 1-2 extended |
| HPIO_L segbits encoding, if step 1 finds no HPIO_RIGHT | New fuzzing campaign; no public source has it |
| Raw `device_info.json` for xcku3p (tile inventory) | Re-run of `extract_device_info.tcl`, keep the output |

## Recommended minimal scope for the builder worker

Stays blocked for tier-1 green. Do not build a full tilegrid to
unblock 55 bits. Do this instead, in order:

1. Run the Q6 step 1 Vivado probe and paste the
   `TILE_TYPE =~ HPIO*` inventory into the run log. If no
   HPIO_RIGHT exists, stop and escalate to the CTO (golden
   change or HPIO_L fuzzing campaign).
2. If HPIO_RIGHT exists, run the Q6 steps 2-3 probe for the
   single row-0 HPIO tile and record its base FAR, frame count,
   and the offset ruling.
3. Add the one-tile table to `resolve_hpio` (harness) and to the
   real `BitgenDevice` impl (emitter), delete
   `HpioProvisionalFallback` and its pinning tests, and correct
   the INT rule to the type-dependent form in Q5 (with the +1
   branch validated by the 76-minor check) in the same change.
4. Re-run the harness. IO_CONFIG goes absolute-provisional at
   best (same status as INT_PIP today) until a full tilegrid
   exists. Report it as such; do not call it tier-1 green.
