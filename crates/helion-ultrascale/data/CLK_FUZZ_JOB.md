# Vivado clocking fuzz job (Job 4, CLK unblocker)

Status: READY TO RUN, not run. License state is UNKNOWN; do not start
until a real Vivado checkout is verified possible (a port answering
is not a checkout; no `lmutil` or Vivado exists on this VM). Do not
run any license-port connect check from this VM.

This job unblocks the CLK block. It replaces the stale Job 4 in
`~/workspace/your_files/vivado-fuzzing-instructions.md` and in
`vivado-fuzzing-package.zip`, which is wrong in four ways:

1. It says "BUFGCE control bits are few" and "Skip for now". The true
   count is zero public lines, and the job is now the CLK unblocker,
   priority after Job 1 (device file). It is not skippable.
2. It has no CLK scripts at all: the package ships TCL/Verilog for
   DSP, LUT, FF, BRAM, and LUTRAM, but nothing for any clocking
   primitive. The scripts in `crates/helion-ultrascale/scripts/clk-fuzz/`
   are new and ship with this job.
3. It does not say what is already grounded. Re-fuzzing grounded
   encodings wastes runs and risks contradicting them. Do NOT fuzz:
   BUFCE_LEAF (all attributes, 832 lines), BUFGCE_HDIO IINV (8 lines),
   CMT HROUTE/HDISTR PIPs and USED bits, or any RCLK PIP/USED bit.
   Those are vendored in `data/` and implemented in `src/clk.rs`.
4. It does not name the audit dumps. This job needs the CMT/RCLK
   tile and site inventory from Vivado (no public tilegrid exists),
   not just bitstream diffs.

No em dashes anywhere.

## Target and flow

- Part: `xcvu3p-ffvc1517-2-e` (confirm with `get_parts`; if the exact
  part string differs, use what `get_parts` reports and note it in
  `runs.txt`). Encodings are tile-type level and are expected to
  transfer to xcku3p (SPEC.md scout report), but the crew re-verifies
  every landed line against the xcku3p `.ll` frame mapping before
  calling it ground truth for xcku3p.
- Standard flow from the package instructions (non-project mode,
  batch mode only): `read_verilog`, `read_xdc`, `synth_design`,
  `opt_design`, `place_design`, `route_design`,
  `write_bitstream -force -raw_bitfile -logic_location_file`.
  Always request `.bit`, `.rbt`, and `.ll` on every run. Keep the
  Vivado version consistent across all runs in the job.
- Method (BitFiltrator paper section V, same as Job 2 for DSP):
  baseline design with one primitive, `DONT_TOUCH`, all attributes
  at defaults; save `baseline.bit/.rbt/.ll`. Then one bitstream per
  non-default attribute value, changing exactly one attribute per
  run. Diff each variant `.rbt` against baseline `.rbt`, scoped to
  the CMT column frames (from the Job 1 device file / the run `.ll`).
  The differing bits are that attribute's encoding.

## Sub-passes (one primitive per design, one run per variant)

Attribute names below come from the UG974 primitive definitions as
named in the site-type JSONs vendored in `data/`
(`site_type_BUFGCE.json` etc.). The TCL driver does NOT trust this
list: before fuzzing an attribute it checks the attribute exists on
the placed cell with `report_property` and logs plus skips any name
Vivado rejects. A skipped name is reported in `runs.txt`, never
silently dropped and never fuzzed under a guessed spelling.

- **BUFGCE** (`scripts/clk-fuzz/bufgce_top.v`): IS_CE_INVERTED,
  IS_I_INVERTED, CE_TYPE (SYNC/ASYNC). Expect few bits; the CMT
  BUFGCE is the golden blinky's clock buffer, so this sub-pass is
  first.
- **BUFGCTRL** (`scripts/clk-fuzz/bufgctrl_top.v`): IS_CE0_INVERTED,
  IS_CE1_INVERTED, IS_I0_INVERTED, IS_I1_INVERTED,
  IS_IGNORE0_INVERTED, IS_IGNORE1_INVERTED, IS_S0_INVERTED,
  IS_S1_INVERTED, PRESELECT_I0, PRESELECT_I1.
- **BUFGCE_DIV** (`scripts/clk-fuzz/bufgce_div_top.v`): IS_CE_INVERTED,
  IS_I_INVERTED, IS_CLR_INVERTED, CE_TYPE, BUFGCE_DIVIDE (1, 2, 3,
  4, 5, 6, 7, 8).
- **BUFCE_ROW / BUFCE_ROW_FSR** (`scripts/clk-fuzz/bufce_row_top.v`):
  the driver enumerates the cell properties at runtime; the site
  JSONs list CEINV/IINV site pips but no attribute list is asserted
  here, because no public source names the BUFCE_ROW primitive
  attributes. Whatever the enumeration finds is fuzzed; whatever it
  does not find is logged as not present.
- **MMCME4_ADV** (`scripts/clk-fuzz/mmcm_top.v`): the driver
  enumerates the placed cell's properties at runtime and fuzzes its
  IS_*_INVERTED booleans; every other property (CLKOUT divide /
  multiply, CLKFBOUT, DIVCLK_DIVIDE, phase, duty, bandwidth,
  compensation) is logged in `runs.txt` as skipped-not-fuzzed, because
  a bare 0/1 value cannot tell a boolean from an integer attribute
  (DIVCLK_DIVIDE reads back as "1"; flipping it to 0 would be an
  invalid divide value). Integer/enum attributes need per-attribute
  value tables, which is future work: SPEC.md keeps MMCM/PLL
  attribute fuzzing out of scope until a design needs it. The run log
  lists every property considered, fuzzed or skipped. This is the
  largest sub-pass; it runs last.
- **PLLE4_ADV** (`scripts/clk-fuzz/pll_top.v`): same runtime
  enumeration as MMCM (IS_*_INVERTED booleans fuzzed, every other
  property logged as skipped-not-fuzzed).

## Audit dumps (no bitstream needed, run first)

Before the fuzz runs, dump the inventory the public sources lack:

```tcl
# save as dump_clk_inventory.tcl, run once per part
set part xcvu3p-ffvc1517-2-e
# CMT tiles and their sites (BUFGCE/MMCM/PLL coordinates)
set fp [open "cmt_tiles.txt" w]
foreach tile [get_tiles -filter {TILE_TYPE =~ CMT*}] {
    puts $fp [get_property NAME $tile]
    foreach site [get_sites -of_objects $tile] {
        puts $fp "  [get_property NAME $site] [get_property SITE_TYPE $site]"
    }
}
close $fp
# RCLK tiles and their BUFCE sites
set fp [open "rclk_tiles.txt" w]
foreach tile [get_tiles -filter {TILE_TYPE =~ RCLK*}] {
    puts $fp [get_property NAME $tile]
    foreach site [get_sites -of_objects $tile] {
        puts $fp "  [get_property NAME $site] [get_property SITE_TYPE $site]"
    }
}
close $fp
# PIP inventory of one CMT tile and one RCLK_INT tile (verify the
# proposed BUFGCE FASM spelling in tests/fixtures/clk_blinky.fasm)
set fp [open "cmt_pips.txt" w]
foreach pip [get_pips -of_objects [get_tiles CMT_L_X0Y0]] {
    puts $fp [get_property NAME $pip]
}
close $fp
```

If a tile coordinate in the script does not exist on the part, verify
with `get_tiles` and use a real one; note the substitution in
`runs.txt`. The inventory dumps confirm the tile/site naming the
golden assumes (CMT_L, BUFGCE_X0Y0) and the proposed BUFGCE FASM
spelling; if Vivado names them differently, the golden and
`src/clk.rs` change to match Vivado, not the other way round.

## What to bring back

For each sub-pass, organized by primitive and run:

1. All `.bit`, `.rbt`, and `.ll` files (baseline plus one variant
   per attribute value).
2. The TCL scripts used for each run (the scripts in
   `scripts/clk-fuzz/` plus any local edits, so results are
   reproducible).
3. `runs.txt`: for each run, the design, the attribute changed and
   its old/new value, the Vivado version, the part string, and
   whether it completed cleanly. A run that fails is reported with
   the log excerpt; it is never silently skipped.
4. `cmt_tiles.txt`, `rclk_tiles.txt`, `cmt_pips.txt` from the audit
   dumps.

## Vendoring (crew side, after the runs land)

- New segbits lines go into `crates/helion-ultrascale/data/` with
  provenance in `PROVENANCE.md` (source: Vivado fuzz run, Vivado
  version and `runs.txt` reference; not CC0 upstream data).
- BUFGCE/BUFGCTRL/BUFGCE_DIV/MMCM/PLL lines live in CMT tile frames,
  so they land as `CMT_L.*` / `CMT_RIGHT.*` lines: a new file
  `segbits_cmt_left.db` (tile type `CMT_L`, no public file exists
  today) plus additions to `segbits_cmt_right.db`. BUFCE_ROW lines,
  if the diff produces any, land in the RCLK file of their tile type.
- Each landed line is cross-checked against its run `.ll` (frame and
  bit position of the placed site) before it is trusted; a line that
  disagrees with the `.ll` is not vendored.
- After vendoring: extend `src/clk.rs` (resolve the new site configs,
  delete the corresponding `MissingEncoding` paths), extend
  `tests/clk.rs` (hand-checked bits per new attribute), re-run the
  full suite plus the harness diff on `clk_blinky`, and update
  `data/CLK_AUDIT.md` with the new inventory. The CLK block goes
  green only on zero diff plus a critic re-run.

## Rules

- Never invent numbers. If a run fails, report the failure with the log.
- Every bitstream must come from a real Vivado run on the cluster.
- Do not modify the test designs between baseline and variant runs
  except for the single attribute being fuzzed.
- Keep the Vivado version consistent across all runs in a job.
- Do not run any license check from the Helion VM; the license is
  verified on the cluster by `vivado -version` succeeding and a real
  checkout completing, not by a port probe.
