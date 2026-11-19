# DSP48E2 config-bit encoding: search record

Status: MISSING in every public source searched. No DSP48E2 config-bit
encoding is derivable from public sources. Do not invent one. Designs
using a DSP48E2 fail loud (`BitgenError::Dsp48e2Blocked`, see
`crates/helion-ultrascale/src/dsp.rs`) until Vivado fuzzing job 2
lands the encoding.

Searched 2026-11-19 by the DSP48E2 worker (session 18), on branch
`fm/ultra-bitgen-dsp`, worktree `~/workspace/helion-ultra-bitgen-dsp`.
Every command below was run, and every output below is the real output
(trimmed only where noted). Nothing here is inferred from memory.

## Sources searched

1. prjuray-db reference clone, `~/workspace/prjuray-db-ref`,
   commit `affbc5e` ("Merge pull request #2 from SymbiFlow/next_zynqusp_db",
   2020-07-21). Its `COPYING` file opens "Creative Commons Legal Code /
   CC0 1.0 Universal". This is the same clone the segbits-vendor worker
   verified (COORD.md, 2026-11-07).
2. BitFiltrator reference clone,
   `~/workspace/helion-ultra-bitstream/bitfiltrator-ref/`, commit
   `79b9bd6` ("Update README with frame offset in bitstream"). MIT
   License, Copyright (c) 2022 EPFL VLSC (same commit the
   segbits-vendor worker verified).
3. The vendored copies under `crates/helion-ultrascale/data/` in this
   worktree (same content as the clones for the files checked).

## 1. prjuray-db has no DSP segbits file

Command:

```sh
ls ~/workspace/prjuray-db-ref/zynqusp/ | grep -i dsp
```

Output (the complete list of DSP-named files under `zynqusp/`):

```text
segbits_rclk_dsp_intf_clkbuf_l.db
segbits_rclk_dsp_intf_clkbuf_l.origin_info.db
segbits_rclk_dsp_intf_l.db
segbits_rclk_dsp_intf_l.origin_info.db
segbits_rclk_dsp_intf_r.db
segbits_rclk_dsp_intf_r.origin_info.db
```

There is no `segbits_dsp*.db`. The full `zynqusp/` segbits listing
contains `segbits_bram*.db`, `segbits_clel_*.db`, `segbits_clem*.db`,
`segbits_cmt_right.db`, `segbits_hdio_*.db`, `segbits_hpio_right.db`,
`segbits_int.db`, `segbits_int_intf_left_term_pss.db`,
`segbits_rclk_*.db`, `segbits_xiphy_byte_right.db`, and nothing for a
DSP tile type.

## 2. The rclk_dsp_intf files carry clock PIPs only, no DSP config

Command:

```sh
grep -c "OPMODE\|ALUMODE\|INMODE\|DSP48" \
  ~/workspace/prjuray-db-ref/zynqusp/segbits_rclk_dsp_intf_l.db \
  ~/workspace/prjuray-db-ref/zynqusp/segbits_rclk_dsp_intf_r.db \
  ~/workspace/prjuray-db-ref/zynqusp/segbits_rclk_dsp_intf_clkbuf_l.db
```

Output:

```text
.../segbits_rclk_dsp_intf_l.db:0
.../segbits_rclk_dsp_intf_r.db:0
.../segbits_rclk_dsp_intf_clkbuf_l.db:0
```

Sample lines (all three files are clock PIPs of this form):

```text
RCLK_DSP_INTF_L.PIP.CLK_BUFCE_ROW_FSR_0_CLK_IN.CLK_CMT_DRVR_TRI_ESD_0_CLK_OUT_SCHMITT_B !04_34 04_41
RCLK_DSP_INTF_L.PIP.CLK_CMT_MUX_2TO1_1_CLK_OUT.CLK_CMT_DRVR_TRI_ESD_2_CLK_OUT_SCHMITT_B !04_33 04_46
```

These are the clock-interface tiles adjacent to DSP columns, not DSP
configuration. Line counts: `segbits_rclk_dsp_intf_l.db` 80,
`segbits_rclk_dsp_intf_r.db` 80,
`segbits_rclk_dsp_intf_clkbuf_l.db` 288.

## 3. site_type_DSP48E2.json documents pins only

Command (Python, on the vendored copy, identical to the clone file):

```python
d = json.load(open('crates/helion-ultrascale/data/site_type_DSP48E2.json'))
sorted(d.keys())
len(d['site_pins']); len(d['site_pips'])
```

Output:

```text
site_type_DSP48E2.json top-level keys: ['site_pins', 'site_pips', 'type']
site_pins type: dict count: 432
site_pips type: dict count: 30
```

`type` is `"DSP48E2"`. There is no segbits, bit-location, or
config-attribute table in the file. The 432 site pins are the DSP48E2
port list; the 30 site pips are internal pin connections. This matches
the PROVENANCE.md note: "site_type_DSP48E2.json documents pins only".

## 4. tile_type_DSP.json documents pips and wires only

Command (Python, on the clone file):

```python
d = json.load(open('.../prjuray-db-ref/zynqusp/tile_types/tile_type_DSP.json'))
sorted(d.keys())
```

Output:

```text
tile_type_DSP.json top-level keys: ['pips', 'sites', 'tile_type', 'wires']
```

There is no `segbits` key. The `pips` map wire-to-pin connections
such as `DSP.DSP_DSP48_0_ACIN_B0->DSP_DSP48_0_ACIN_B0_PIN` (routing
into the site pins), with `can_invert`, `is_directional`,
`is_pseudo`, `src_wire`, `dst_wire` per pip. No config bits.

## 5. BitFiltrator ULTRASCALE_PLUS.json has no DSP table at all

Command (Python, on both the vendored copy and the reference clone):

```python
d = json.load(open('crates/helion-ultrascale/data/ULTRASCALE_PLUS.json'))
list(d.keys())
```

Output (identical for the reference clone file
`bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json`):

```text
['BRAM', 'CLEL_L', 'CLEL_R', 'CLEM', 'CLEM_R']
```

Command:

```sh
grep -c "DSP48E2\|DSP_" crates/helion-ultrascale/data/ULTRASCALE_PLUS.json
```

Output: `0`. A case-insensitive count of `DSP` over the whole
reference-clone arch JSON is likewise 0.

What BitFiltrator does have for DSP is frame addressing, not config
encodings: `resources/devices/xcku3p.json` carries `num_dsps: 1368`
and a `dsp_colMajors` map (column to major-frame numbers), and
`src/arch_spec.py` has `num_dsp_per_column()`. Column majors locate
the DSP columns in the bitstream; they say nothing about which bits
configure `AREG`, `USE_MULT`, or any other attribute. The
`src/tcl/dsp.v` template and `src/tcl/one_dsp_in_every_dsp_column.tcl`
are experiment scaffolding for producing such data with Vivado, not
data themselves.

## 6. No other file in either clone mentions a DSP encoding

Command:

```sh
grep -ril "DSP48E2" ~/workspace/prjuray-db-ref/zynqusp/
```

Output (complete):

```text
.../zynqusp/site_types/site_type_DSP48E2.json
.../zynqusp/tile_types/tile_type_DSP.json
.../zynqusp/xczu3eg-sbva484-1-e/tilegrid.json
.../zynqusp/xczu3eg-sfvc784-1-e/tilegrid.json
```

The `tilegrid.json` hits are site names (`DSP48E2_X0Y0`,
`DSP48E2_X0Y1`, ...) and the tile type name (`DSP`), i.e. placement
coordinates, not encodings. `grep -ri "dsp"` over the vendored
segbits DBs (`crates/helion-ultrascale/data/segbits_*.db`) finds no
DSP feature lines at all.

## 7. What exists in the crate today

- `BelKind::Dsp48e2` (`src/bel.rs`): the BEL exists in the device
  model; it carries no encoding.
- `src/bin/helion-bitgen.rs` `block_for_feature`: labels a
  `DSP48E2` tile/feature as block `DSP48E2` for the diff harness.
  Labelling only; it emits nothing.
- `src/dsp.rs` (this branch): the fail-loud detector plus the
  `BitgenError::Dsp48e2Blocked` error, wired into `emit_frames`
  before any segbits lookup. Regression tests in
  `tests/dsp48e2.rs`.

## Missing data, exactly

A per-attribute config-bit map for DSP48E2, in the same shape as the
existing `LutLoc` tables: for each of the 46 primitive attributes in
the job-2 table (`dsp_variant_table.tsv`, taken from the BitFiltrator
`dsp.v` template), the list of (minor, frame_ofst) pairs, and the
value-to-bit mapping per attribute value, scoped to the DSP column
frames (CLB_IO_CLK block type, per the scout report). Note
`src/dsp.rs` `DSP48E2_ATTRIBUTES` lists 43 of the 46 by bare name:
`RND`, `MASK`, and `PATTERN` are excluded there because they are
generic words, not because they are unfuzzed.

## Unblocker, exactly

Vivado fuzzing job 2 (DSP48E2 attribute sweep), as specified in
`docs/ultra-bitgen-crew/fuzzing-job2-dsp48e2/JOB2.md` on this branch:
one baseline bitstream (single DSP48E2, all attributes at defaults,
fixed placement) plus one variant bitstream per non-default attribute
value, all on part `xcvu3p-ffvc1517-2-e`, each with `.bit`, `.rbt`
(ASCII bitstream, for diffing), and `.ll` (logic location file).
The job needs a working Vivado license checkout; the license server
`27000@license2.forest.usf.edu` has refused connections since
Oct 22, 2026, and license state is UNKNOWN from this VM. Do not run
any license-port connect check from this VM: a TCP connect proves
nothing, and the license state is recorded as UNKNOWN.

## Where the job-2 data gets vendored

When job 2 completes, its derived encoding lands under
`crates/helion-ultrascale/data/` as `segbits_dsp.db` (prjuray-style
segbits lines, one per attribute value) and/or a `DSP48E2` section in
the BitFiltrator-style tables, with provenance (source, Vivado
version, part string, run manifest) recorded in
`crates/helion-ultrascale/data/PROVENANCE.md`, the single provenance
index. The raw `.bit`/`.rbt`/`.ll` files are working data, not
vendored: only the derived encoding plus the run manifest are.
