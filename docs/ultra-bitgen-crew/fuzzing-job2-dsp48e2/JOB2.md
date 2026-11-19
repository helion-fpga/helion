# Vivado fuzzing job 2: DSP48E2 config bits

Status: READY TO RUN, blocked on a verified Vivado license checkout.
License server `27000@license2.forest.usf.edu` has refused connections
since Oct 22, 2026; license state is UNKNOWN from this VM. Do not run
any license-port connect check from this VM: a TCP connect proves
nothing. The driver (see `run_job2.sh`) checks the license the only
honest way, by running Vivado: if `vivado -mode batch` cannot check
out a license, the first run fails loudly and the job stops.

Part: `xcvu3p-ffvc1517-2-e` (confirm with `get_parts` in Vivado; if the
exact part string differs, use what `get_parts` reports and note it in
`runs.txt`).

## Goal

Learn which bitstream bits configure each DSP48E2 attribute, so Helion
can emit DSP48E2 configuration bits that diff to zero against Vivado.

## Method (BitFiltrator paper section V)

1. Baseline (v000): one DSP48E2 with DONT_TOUCH, all attributes at
   defaults, placed at one fixed site. One bitstream.
2. For each of the 46 DSP48E2 attributes, one bitstream per
   non-default value (55 variants, `dsp_variant_table.tsv`).
   Each variant differs from the baseline in exactly one attribute.
3. Diff each variant `.rbt` against the baseline `.rbt`. The
   differing bits, scoped to the DSP column frames, are that
   attribute value's encoding.

Total: 56 Vivado runs (1 baseline + 55 variants). The instructions
estimate 60-100 runs; 56 is the single-attribute sweep. Follow-up
combined runs (below) are extra and are logged separately.

## Files in this directory

| File | What it is |
|---|---|
| `dsp_variant_table.tsv` | The sweep table: 46 attributes, 55 variants, defaults and variant values. Single source of truth. |
| `gen_dsp_variants.py` | Generates `variants/v000..v055/dsp_variant.v` plus `manifest.tsv` from the table. Verified 2026-11-19 by the replacement worker: the baseline is identical to the BitFiltrator `dsp.v` template on all 96 `.NAME(value)` lines (46 attributes plus 50 port connections, zero diffs), and each variant differs from the baseline in exactly one attribute line (plus the two comment header lines naming the run and the override, which do not affect synthesis). |
| `pick_dsp_site.tcl` | Resolves one fixed DSP48E2 site (lowest site in dictionary order) for all runs. Run once. |
| `run_dsp_variant.tcl` | One Vivado run for one variant: non-project flow per the instructions template, `place_cell` to the fixed site, `write_bitstream -force -raw_bitfile -logic_location_file`. |
| `run_job2.sh` | Driver: resolves the site once, runs v000..v055 in order, checks every output exists, appends to `runs.txt`, stops on the first failure. |
| `JOB2.md` | This file. |

## How to run (on the cluster)

```sh
python3 gen_dsp_variants.py dsp_variant_table.tsv variants
./run_job2.sh xcvu3p-ffvc1517-2-e ./out
```

Runs are sequential, never parallel. Keep the Vivado version
consistent across all runs in the job; record it in `runs.txt`
(the driver copies it from each run's `run_info.txt`).

## Exact outputs required per run

In `out/<run_id>/`, each run must produce all of:

- `dsp.bit` (raw bitstream)
- `dsp.rbt` (ASCII bitstream: the diff input)
- `dsp.ll` (logic location file: maps the placed DSP to its frames)
- `post_route.dcp` (placed and routed checkpoint, for re-analysis)
- `run_info.txt` (part, variant Verilog path, DSP site, Vivado version)
- `vivado.log` (full Vivado log for that run)

Plus, once per job, in `out/`:

- `site.txt` (the fixed DSP site, one line)
- `runs.txt` (one line per run: run id, attribute, variant value,
  part, site, Vivado version, status OK/FAIL; plus job start/finish
  lines). A failed run is recorded with its failure, and the job
  stops: never silently skip a failed run, never proceed past it.

## What a zero diff means

A variant whose `.rbt` is identical to the baseline is a RESULT, not
a failure. It means that attribute value produces no config-bit
change in isolation (for example, `XORSIMD` only takes effect when
`USE_WIDEXOR` is `TRUE`, and `MASK` only matters when pattern detect
is on). Record the zero diff in the analysis. Do not silently re-run
the variant combined with its enabling attribute as the primary run.
If a combined run is wanted, log it in `runs.txt` as a follow-up run
with both attributes named, and keep the single-attribute run as the
primary result.

## Analysis (done by the Helion crew after the job, not on the cluster)

1. For each variant, diff `dsp.rbt` against the baseline `dsp.rbt`.
2. Scope the diff to the DSP column frames, using the Job 1 device
   file (`dsp_colMajors` in `resources/devices/xcvu3p.json`).
   DSP bits live in CLB_IO_CLK frames, not a separate block type
   (scout report section 2).
3. Cross-check each differing bit against the variant's `.ll` entry
   for the DSP cell.
4. Emit, per attribute value, the list of (minor, frame_ofst) pairs
   plus the value-to-bit mapping, in the same schema as the existing
   `LutLoc` tables.

## Where the data gets vendored

The derived encoding lands under
`crates/helion-ultrascale/data/` as `segbits_dsp.db` (prjuray-style
segbits lines, one per attribute value) and/or a `DSP48E2` section in
the BitFiltrator-style tables, with provenance (source, Vivado
version, part string, run manifest) recorded in
`crates/helion-ultrascale/data/PROVENANCE.md`, the single provenance
index. The raw `.bit` / `.rbt` / `.ll` files are working data, not
vendored: only the derived encoding plus the run manifest are.
Once vendored, the `Dsp48e2Blocked` fail-loud path in
`crates/helion-ultrascale/src/dsp.rs` is replaced by real emission,
with regression tests and a DSP golden design diffed to zero.

## Fixes vs the shipped fuzzing package

The shipped package (`~/workspace/your_files/vivado-fuzzing-package.zip`,
`instructions.md` identical to
`~/workspace/your_files/vivado-fuzzing-instructions.md`) had these
gaps for job 2, all fixed on this branch:

- F1 (missing): no job-2 sweep script existed at all. The
  instructions described the method in prose only. This directory
  adds the table, the generator, the site picker, the per-variant
  driver, and the job driver.
- F2 (wrong): the package's only DSP script,
  `one_dsp_in_every_dsp_column.tcl`, writes the bitstream WITHOUT
  `-raw_bitfile` and WITHOUT `-logic_location_file`, so it produces
  neither the `.rbt` the diff method needs nor the `.ll` that maps
  the DSP to its frames. `run_dsp_variant.tcl` requests both on
  every run, per the instructions ("ALWAYS request both on every
  run").
- F3 (divergent): the same script disables CRC
  (`BITSTREAM.GENERAL.CRC DISABLE`) and compression, diverging from
  the instructions' standard flow template, which sets neither.
  `run_dsp_variant.tcl` sets neither, so the only difference between
  two runs is the fuzzed attribute.
- F4 (stale): the same script's header comment names the script
  `one_dsp_in_every_clb_column.tcl` and the project
  `one_dsp_in_every_clb_column` (copy-paste from the CLB script).
  Cosmetic; not carried into the new driver.
- F5 (unpinned): the instructions say "roughly 40-60 attributes"
  with no list. The table pins the exact 46 attributes and 55
  variant values, with defaults copied from the `dsp.v` template
  comments.

`one_dsp_in_every_dsp_column.tcl` itself is a Job 1 script (device
file: one DSP per DSP column), not a Job 2 script, and is unchanged.
Its F2/F3 issues matter for Job 1 reuse too; if Job 1 re-runs that
script, add `-raw_bitfile -logic_location_file` and drop the CRC /
compression overrides the same way.

## Scope notes

- Dynamic control ports (`OPMODE`, `ALUMODE`, `INMODE`,
  `CARRYINSEL`) are live inputs, not primitive attributes. They have
  no sweep entry: their values are routing, not DSP config bits.
  Job 2 fuzzes the 46 primitive attributes only.
- `RND`, `MASK`, `PATTERN` are 48-bit constants. The variant values
  (`1`, `0`, `1`) are minimal single-bit changes to the constant, so
  the diff stays attributable to the attribute.
- `IS_*_INVERTED` variants flip only the least significant bit of
  each inversion vector (for example `IS_ALUMODE_INVERTED`
  `4'b0000` to `4'b0001`), again to keep the diff minimal.
- Do not modify a variant Verilog between runs. The generator output
  is the input to every run for that id.

## Interface to the synthesis-mapping worker (GAP-F1)

The sibling worker on `fm/ultra-bitgen-synthmap` owns synthesis
mapping to UltraScale primitives, including DSP48E2. The interface
between that work and this block is:

- Vocabulary: the mapper must use exactly the 46 attribute names in
  the job-2 table (`dsp_variant_table.tsv`, same list as
  `gen_dsp_variants.py` `EXPECTED_ATTRIBUTES`, taken from the `dsp.v`
  template). Job 2 fuzzes exactly that set. Any attribute the mapper
  emits that job 2 did not fuzz stays blocked. Note the count differs
  from `DSP48E2_ATTRIBUTES` in `crates/helion-ultrascale/src/dsp.rs`
  on purpose: that list has 43 names, the bare-name detector subset.
  `RND`, `MASK`, and `PATTERN` are excluded there because they are
  generic words, and a bare feature of that name on a non-DSP tile
  must stay `UnknownFeature`; they are still fuzzed by job 2, and
  they still block whenever the tile or feature itself names DSP48E2
  or a DSP tile.
- FASM: the mapper emits DSP48E2 cells as FASM entries naming a
  DSP48E2 site/tile and its attributes. The emitter is the
  enforcement point: until the job-2 encoding lands, any such entry
  fails with `BitgenError::Dsp48e2Blocked`, which names the missing
  encoding and the unblocker.
- No workaround: the mapper must not invent DSP bits, and must not
  silently drop a DSP48E2 cell (or its attributes) to make a design
  emit. A design that needs a DSP either maps it and blocks loudly
  at bitgen, or is out of scope for that run. Dropping a cell to
  force emission would be a miscompile.
- Coordination goes through the orchestrator only. Neither worker
  edits the other's branch. (Note: `fm/qor2-mapper` commit `47eed1e`
  contains a proven miscompile with a verified fix uncommitted;
  branch repair there is a pending CTO decision. Do not touch
  `fm/qor2-mapper` without that ruling.)

## Rules

- Never invent numbers. If a run fails, report the failure with the
  log. A zero diff is a result, not a failure.
- Every bitstream must come from a real Vivado run on the cluster.
- Do not modify the test designs between baseline and variant runs
  except for the single attribute being fuzzed. Placement is fixed
  to one site for all runs.
- Keep the Vivado version consistent across all runs in the job.
- If the exact part string differs from `xcvu3p-ffvc1517-2-e`, use
  what `get_parts` reports and note it in `runs.txt`.
- Never claim Vivado-equivalence without a real Vivado diff at zero.
