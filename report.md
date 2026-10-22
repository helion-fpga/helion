# Bitstream Scout Report: Virtex UltraScale+ (xcvu3p)

Date: 2026-10-22. Scout: bitstream agent. Worktree:
~/workspace/helion-ultra-bitstream, branch fm/ultra-bitstream.
Reference clone: bitfiltrator-ref/ (github.com/epfl-vlsc/bitfiltrator, depth 1).

## Verdict: GO

Full license-free bitgen for xcvu3p is achievable without AMD's help. The
encoding data needed exists in two public sources, and the gaps are closable
with published fuzzing methodology. The one-time cost is Vivado access for DB
generation (fuzzing runs only); the shipped tool and all user runs stay
license-free.

This verdict is stronger than the brief assumed. The brief said "full PIP
encoding is NOT yet documented." That is outdated: Project U-Ray's public
database (github.com/f4pga/prjuray-db, CC0 1.0) contains interconnect PIP
encodings in FASM format, plus DSP/IO/clock site data. Details below.

## Sources inventoried

1. **BitFiltrator** (EPFL, FPL 2022 paper + repo). Scope per the paper
   abstract: "initial LUT equations, LUTRAM contents, BRAM contents, and
   register values in Xilinx UltraScale and UltraScale+ FPGAs." Paper file:
   bitfiltrator-ref/fpl22-bitfiltrator-paper.pdf. Database:
   bitfiltrator-ref/resources/.
2. **Project U-Ray DB** (f4pga/prjuray-db, CC0 1.0). Per-tile segbits in FASM
   format plus site-type JSONs. Cloned to /tmp/prjuray-db for inspection
   (not committed to the worktree; it is a 275-file reference).
3. **AMD UG570** (UltraScale Architecture Configuration, cited by
   BitFiltrator src/arch_spec.py for the FAR layout, table 9-21).

## Coverage matrix

All paths below are relative to the reference clones named above.

| Resource | Status | Exact pointer |
|---|---|---|
| LUT INIT (64 bits x 8 LUTs/slice) | COVERED | bitfiltrator-ref/resources/archs/ULTRASCALE_PLUS.json: `{CLEL_L,CLEL_R,CLEM,CLEM_R}.LutLoc.Y_ofst[<0..59>].frame_ofst[{A,B,C,D,E,F,G,H}6LUT]` (list of 64 frame offsets) and `.minor[...]` (list of 64 minor addresses). Verified: 8 LUT names, 64 bits each. |
| FF init/config (16 FFs/slice) | COVERED | Same file: `{CLEL_L,CLEL_R,CLEM,CLEM_R}.RegLoc.Y_ofst[<0..59>].frame_ofst[{A..H}FF, {A..H}FF2]` and `.minor[...]`. Verified: 16 FF names. |
| LUTRAM contents | COVERED (same bits) | Paper section V-B: LUTRAM uses the same INIT bits as LUTs (SRL16/32 mode interprets them as shift data). Same LutLoc entries apply. |
| BRAM content (16384 bits) | COVERED | Same file: `BRAM.BramMemLoc.Y_ofst[<0..23>].frame_ofst` (16384 offsets) and `.minor[...]`. |
| BRAM parity (2048 bits) | COVERED | Same file: `BRAM.BramMemParityLoc.Y_ofst[<0..23>].frame_ofst` (2048 offsets) and `.minor[...]`. |
| INT PIP/routing muxes | PARTIAL | /tmp/prjuray-db/zynqusp/segbits_int.db: 3778 lines, FASM format `INT.PIP.<src>.<dst> <bit> !<bit> ...` (e.g. `INT.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1 64_00 !66_00 !68_00 !72_00 !72_01 !74_00`). Bit format is `<frame>_<bit>` with `!` prefix meaning the bit must read 0. Completeness unverified: 3778 PIPs may not cover every mux in an INT tile. |
| DSP48E2 config bits | MISSING (pins only) | /tmp/prjuray-db/zynqusp/site_types/site_type_DSP48E2.json documents site pin names and directions only, no config-bit encoding. BitFiltrator has experiment scaffolding (src/tcl/dsp.v, src/tcl/one_dsp_in_every_dsp_column.tcl) but ships no DSP data. |
| IOB/IO config bits | PARTIAL | /tmp/prjuray-db/zynqusp/segbits_hpio_right.db (1560 lines), segbits_hdio_top_right.db, segbits_hdio_bot_right.db exist. Encoding of specific attributes (IOSTANDARD, DRIVE, SLEW) not verified. |
| Clocking (BUFGCE, MMCM) | PARTIAL | /tmp/prjuray-db/zynqusp/segbits_cmt_right.db (104 lines, mostly CLK_HROUTE PIPs). MMCM/PLL attribute encoding not present. Site types exist: site_type_BUFGCE.json, site_type_BUFGCTRL.json, site_type_MMCME4_ADV.json (verify filename in repo). |
| URAM288 | MISSING | No URAM entries in either database. BitFiltrator BRAM section covers RAMB18/36 only. |
| Frame addressing | COVERED (arch-level) | BitFiltrator src/arch_spec.py, class UltraScalePlusSpec (cites UG570 table 9-21). See FAR section below. |
| xcvu3p device layout | MISSING | BitFiltrator resources/devices/ has no xcvu3p file (34 files, closest is xcku3p, same architecture, different grid). prjuray-db/zynqusp/ is Zynq US+ only. The xcvu3p tile grid, column majors, and FAR row/col ranges must be generated. |

### How to read the BitFiltrator pointers

`Y_ofst` is the slice index within a 60-slice CLB column (0..59). For a BEL
named e.g. `SLICE_X23Y45/B6LUT`: X selects the column major via the device
file's `clb_colMajors`, Y selects the row major and `Y_ofst = Y mod 60`, tile
type comes from `clb_tileTypes[X]`, then the arch file gives 64
(frame_offset, minor) pairs. The full frame address is assembled from block
type CLB_IO_CLK, row, column major, and minor per the FAR layout below.
This resolution flow is implemented in bitfiltrator-ref/src/bit_locator.py
and demonstrated in src/demo.py.

## FAR layout (UltraScale+, from BitFiltrator src/arch_spec.py)

32-bit Frame Address Register, MSB to LSB:

- Bits 31:27: reserved (5 bits)
- Bits 26:24: block type (3 bits): 0 = CLB_IO_CLK, 1 = BRAM_CONTENT,
  2-7 reserved (enum FarBlockType in src/frame_spec.py)
- Bits 23:18: row address (6 bits, clock-region row in bitstream coordinates)
- Bits 17:8: column address (10 bits, major column)
- Bits 7:0: minor address (8 bits, frame index within the column)

Frame size: 93 32-bit words (UltraScalePlusSpec.frame_size_words). Note this
differs from 7-series (101 words).

Device files add: per-SLR IDCODE and config order, clock-region to FAR-row
mapping (including hidden rows: max_far_row_idx can exceed the visible
clock-region count; see resources/README.md), and per-row-major column maps
(bram_content_colMajors, bram_reg_colMajors, clb_colMajors, clb_tileTypes).

## Fuzzing plans for the missing pieces

All plans follow the BitFiltrator published methodology (paper sections V-VI):
baseline bitstream plus single-bit (or single-feature) variants generated by
Tcl-driven Vivado runs, then diff against baseline to isolate the encoding.
One-time Vivado access is required for DB generation only. The shipped
bitgen uses the resulting static tables and never invokes Vivado.

### 1. xcvu3p device layout (highest priority, blocks everything else)

- Tool: bitfiltrator-ref/src/create_device_summary.py, invoked as
  `python3 create_device_summary.py <part>` with part=xcvu3p-<package>-<speed>.
- What it does (paper section IV): generates an empty design, one-FF-per-CLB-column,
  one-BRAM-per-BRAM-column, one-DSP-per-DSP-column designs via
  src/tcl/one_ff_in_every_clb_column.tcl etc., builds bitstreams in Vivado,
  and maps FARs to physical X/Y coordinates. Output: a new
  resources/devices/xcvu3p.json with the same schema as xcku3p.json.
- Designs needed: ~4 (empty, FF columns, BRAM columns, DSP columns). Each is
  one Vivado bitstream run.
- Validation: the paper's section VI experiment (populate all BELs, randomize
  INIT, reconstruct from bitstream, compare) should be rerun for xcvu3p.

### 2. DSP48E2 config bits

- Baseline: instantiate one DSP48E2 with DONT_TOUCH, all attributes at
  defaults, following src/tcl/dsp.v as the template. Generate baseline
  bitstream from the placed design checkpoint.
- Fuzz: for each attribute (AMULTSEL, BMULTSEL, USE_MULT, USE_SIMD,
  USE_WIDEXOR, pattern-detect MASK/PATTERN, IS_*_INVERTED, etc.), generate
  one bitstream per non-default value, diff against baseline. Attribute
  count is on the order of 40-60, so 60-100 Vivado bitstream runs.
- Note: DSP bits live in CLB_IO_CLK frames (like the paper's BRAM register
  note: control bits are not in a separate block type). The diff must be
  scoped to the DSP column's frames using the device file from plan 1.
- Output: new `DSP48E2` section in the arch JSON with the same
  frame_ofst/minor list schema as LutLoc.

### 3. INT PIP completeness verification

- The 3778 PIPs in segbits_int.db need a completeness audit, not fresh
  fuzzing: enumerate the PIPs of one INT tile from Vivado
  (`get_pips -of_objects [get_tiles INT_X<x>Y<y>]`) and check coverage.
- For any missing PIP: baseline design routing that PIP via
  `set_property FIXED_ROUTE` in Tcl, generate bitstream, diff. This is the
  Project X-Ray segbits methodology (f4pga.readthedocs.io prjxray DB dev
  process), applied per missing PIP.
- Estimated missing count is unknown until the audit runs; budget for it as
  the long tail of this workstream.

### 4. URAM288

- No public data found. Plan: one-URAM-per-column design (mirroring
  one_bram_in_every_bram_column.tcl), then content sweep like the BRAM
  experiment (paper section V-A: 422,368 bits per BRAM column; URAM is
  288Kb so expect proportionally more). This is the largest single fuzzing
  job in the plan.
- Lower priority: URAM can be left unconfigured (tied off) in early bitgen;
  designs that need URAM fail loud until this lands.

### 5. IO and clocking attributes

- IO: extend the segbits_hpio_right.db audit the same way as INT PIPs;
  attribute-level fuzzing (IOSTANDARD, DRIVE, SLEW, IN_TERM) per site type.
- Clocking: BUFGCE/CE control bits are few; MMCME4_ADV has dozens of
  attributes and warrants its own fuzzing pass. Defer MMCM until a design
  needs it; tie MMCM tiles to a safe default in early bitgen.

## Route-to-bitgen handoff format

The router emits FASM-like text, one line per configured feature, using the
prjuray-db segbits vocabulary. This matches what the existing open tooling
consumes and keeps the bitgen emitter a pure table lookup.

```
# comment lines start with #
<tile>.<feature> [= <value>]
```

Where `<tile>` is the Vivado tile name (e.g. `INT_X12Y45`, `SLICE_X23Y45`,
`DSP48E2_X2Y10`), and `<feature>` is the segbits feature string exactly as
it appears in the segbits DBs (e.g. `PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1`,
`A6LUT.INIT[63:0]`, `AFF.INIT`, `RAMB18_X2Y12.INIT[16383:0]`).

Examples:

```
INT_X12Y45.PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1
SLICE_X23Y45.A6LUT.INIT = 64'h6996966996696996
SLICE_X23Y45.AFF.INIT = 1'b0
RAMB36_X4Y20.INIT[32767:0] = 32768'h0
```

Bitgen resolution order per line:
1. Split tile and feature. Look up the tile type's segbits DB
   (e.g. INT -> segbits_int.db).
2. Find the feature line; for each `<frame>_<bit>` token, set the bit in the
   addressed frame (`!` prefix means the bit must be 0; assert it reads 0 in
   the base frameset).
3. For INIT-style features, expand the value MSB-first across the listed
   bits (bit 0 = LSB of the value).
4. Frame addresses come from the device file (plan 1) combined with the
   arch-level (frame_offset, minor) pairs.

The INTERFACES.md entry for this contract is filed separately on day 2 per
the brief. The format above is the proposal the critique agent reviews.

## Go/no-go: GO

Closable without AMD's help. The encoding data exists in public sources for
LUT, FF, BRAM, and substantially for interconnect; the remainder follows a
published, reproducible methodology. The critical path is: (1) generate the
xcvu3p device file (one-time Vivado, ~4 bitstream runs), (2) audit INT PIP
completeness, (3) fuzz DSP48E2 attributes. URAM and MMCM can trail.

## Legal note

The extracted parameters are facts about a file format: which bit positions
in a configuration frame control which named resource. They are discovered
by observing the outputs of a tool (Vivado-generated bitstreams), not by
copying AMD code or data files. This is the same theory Project X-Ray,
Project Trellis, and Project IceStorm operate under, and the U-Ray database
is explicitly released under CC0 1.0 (public domain dedication). No AMD
binaries or license servers are touched beyond the documented one-time DB
generation step, which uses a legitimately licensed Vivado install to
produce test vectors.

## Open questions for the orchestrator

1. Who provides the one-time Vivado access for xcvu3p DB generation, and on
   what machine? The sandbox has no Vivado.
2. prjuray-db/zynqusp is Zynq US+; the INT tile PIP encoding is expected to
   be identical across the US+ family (tile types are shared), but this
   assumption needs one confirmation run on a Virtex part when Vivado
   access exists.
3. The /tmp/prjuray-db clone is reference only and not committed. If the
   crew wants it vendored, that is a CTO decision (CC0 permits it).
