# Vivado fuzzing job 5: URAM288 config and content bits

Status: READY TO RUN, blocked on a verified Vivado license checkout.
The Vivado license server has refused connections
since Oct 22, 2026; license state is UNKNOWN from this VM. Do not run
any license-port connect check from this VM: a TCP connect proves
nothing. The driver (see `run_job5.sh`) checks the license the only
honest way, by running Vivado: if `vivado -mode batch` cannot check
out a license, the first run fails loudly and the job stops.

Part: `xcvu3p-ffvc1517-2-e` (confirm with `get_parts` in Vivado; if the
exact part string differs, use what `get_parts` reports and note it in
`runs.txt`).

## Goal

Learn which bitstream bits configure each URAM288 attribute and which
bits hold each of the 294,912 content bits, so Helion can emit URAM288
configuration and content bits that diff to zero against Vivado.

## Why this job has three passes

URAM288 differs from DSP48E2 in one structural way, established in
`crates/helion-ultrascale/data/URAM288_SEARCH.md`: the primitive has
34 attributes in UG974 and no `INIT_xxx` attribute at all. A BRAM
content sweep (vary one INIT word per run) cannot be copied, because
there is no INIT word to vary. The content path is the XPM memory
initialization file instead (UG974 XPM_MEMORY_SPRAM), and the
attribute sweep and the content sweep need different designs. Pass C
(one URAM per column) exists because no public source states how many
URAM columns xcvu3p has or which frames each column occupies.

## Method (BitFiltrator paper section V)

1. Pass A, attribute sweep (39 runs): baseline (v000), one URAM288
   with DONT_TOUCH, all attributes at UG974 defaults, placed at one
   fixed site. For each of the 34 URAM288 attributes, one bitstream
   per non-default value (v001..v038, `uram_variant_table.tsv`; 38
   variants because CASCADE_ORDER_A/B have three values each).
   Each variant differs from the baseline in exactly one attribute.
2. Pass B, content probes (13 runs): baseline (p000), one URAM288
   through an xpm_memory_spram with `MEMORY_PRIMITIVE` "ultra" and
   an all-zero `.mem` file, placed at the same fixed site. Twelve
   probes (p001..p012, `gen_uram_content_probes.py`) vary the `.mem`
   contents at chosen words and bits. p012 is the control: every word
   all 72 bits set. If its `.rbt` equals the baseline `.rbt`, the init
   file did not take effect and pass B is void (see below).
3. Pass C, one URAM per column (1 run): one URAM288 in each URAM
   column, all at UG974 defaults. The `.ll` file maps every column's
   cell to its frames.

Total: 53 Vivado runs (39 + 13 + 1). The instructions estimate
60-100 runs for job 2; 53 is the single-attribute plus probe sweep
for URAM. Follow-up combined runs are extra and are logged
separately.

## Files in this directory

| File | What it is |
|---|---|
| `uram_variant_table.tsv` | The pass-A sweep table: 34 attributes, 38 variants, defaults and variant values. Single source of truth. |
| `gen_uram_variants.py` | Generates `variants/v000..v038/uram_variant.v` plus `manifest.tsv` from the table. Self-checks that every variant differs from the baseline in exactly one attribute line. |
| `gen_uram_content_probes.py` | Generates `probes/p000..p012/probe.mem` plus `uram_probe.v` plus `manifest.tsv`. Self-checks the `.mem` shape (4096 lines of 18 hex digits). |
| `gen_uram_column_design.py` | Generates `column_design.v` (one URAM288 per column) from `column_sites.txt`. |
| `pick_uram_site.tcl` | Resolves one fixed URAM288 site (lowest site in dictionary order) for passes A and B. Run once, by the driver. |
| `pick_uram_columns.tcl` | Resolves one URAM288 site per URAM column (lowest site per X) for pass C. Run once, by the driver. |
| `run_uram_variant.tcl` | One Vivado run for one pass-A variant: non-project flow, `place_cell` to the fixed site, `write_bitstream -force -raw_bitfile -logic_location_file`. |
| `run_uram_content_probe.tcl` | One Vivado run for one pass-B probe: non-project flow, `read_verilog` plus `read_mem`, same placement and bitstream flags. |
| `run_uram_column.tcl` | One Vivado run for pass C: places each `URAM288_col<i>` at the i-th site in the site list. |
| `run_job5.sh` | Driver: resolves the site once, runs pass A, then pass B, then the pass-B control check, then pass C; checks every output exists, appends to `runs.txt`, stops on the first failure. |
| `JOB5.md` | This file. |

## How to run (on the cluster)

```sh
python3 gen_uram_variants.py uram_variant_table.tsv variants
python3 gen_uram_content_probes.py probes
./run_job5.sh xcvu3p-ffvc1517-2-e ./out
```

Runs are sequential, never parallel. Keep the Vivado version
consistent across all runs in the job; record it in `runs.txt`
(the driver copies it from each run's `run_info.txt`).

## Exact outputs required per run

In `out/<run_id>/`, each run must produce all of:

- `uram.bit` (raw bitstream)
- `uram.rbt` (ASCII bitstream: the diff input)
- `uram.ll` (logic location file: maps the placed URAM to its frames)
- `post_route.dcp` (placed and routed checkpoint, for re-analysis)
- `run_info.txt` (part, variant Verilog or probe dir path, URAM site, Vivado version)
- `vivado.log` (full Vivado log for that run)

Plus, once per job, in `out/`:

- `site.txt` (the fixed URAM site, one line)
- `column_sites.txt` (one URAM site per column, one line each)
- `column_design.v` (the generated pass-C design)
- `runs.txt` (one line per run: run id, attribute/probe, variant
  value, part, site, Vivado version, status OK/FAIL; plus job
  start/finish lines and the pass-B control line). A failed run is
  recorded with its failure, and the job stops: never silently skip
  a failed run, never proceed past it.

## What a zero diff means

A variant whose `.rbt` is identical to the baseline is a RESULT, not
a failure. It means that attribute value produces no config-bit
change in isolation (for example, `AVG_CONS_INACTIVE_CYCLES` is
power estimation only and may set no bit, and a non-NONE
`CASCADE_ORDER` on a lone URAM may be a no-op or a DRC; a DRC is a
failed run with a log, not a zero diff). Record the zero diff in
the analysis.
Do not silently re-run the variant combined with its enabling
attribute as the primary run. If a combined run is wanted, log it in
`runs.txt` as a follow-up run with both attributes named, and keep
the single-attribute run as the primary result.

## The pass-B control (read this before analyzing pass B)

p012 sets every word to all 72 bits. The driver compares
`out/p012/uram.rbt` against `out/p000/uram.rbt` (all zero) after
pass B and stops with CONTROL-FAIL if they are identical. An
identical pair means the `.mem` file did not reach the bitstream
(synthesis ignored the init file, mapped the XPM macro to something
other than a URAM288, or the file was never read), and no pass-B
diff can be trusted. Do not analyze pass-B diffs after a
CONTROL-FAIL; report the failure with the logs and fix the init
mechanism first. The most likely causes, in order, are: the `.mem`
file was not found (it is named by value only, `"probe.mem"`, per
UG974; the run happens with the probe directory as the working
context, and `read_mem` is given the full probe path), or the Vivado
version maps this XPM configuration to block RAM instead of UltraRAM
(the per-run cell-count assertion catches that case before the
control even runs).

## Analysis (done by the Helion crew after the job, not on the cluster)

1. Pass A: for each variant, diff `uram.rbt` against the baseline
   `uram.rbt`. The differing bits, scoped to the URAM column frames
   (from the pass-C `.ll` file and the Job 1 device file), are that
   attribute value's encoding.
2. Pass B: for each probe, diff `uram.rbt` against the p000
   `uram.rbt`. The differing bits are the encoding of the probed
   word/bit positions. The probe set is designed to reveal structure:
   p001 vs p005 vs p006 vs p009 test whether the word index maps to
   consecutive frames or interleaved ones; p001 vs p002 vs p003 test
   the bit order inside a word (bit 0, bit 64, bit 71); p010 vs p011
   test whether one bit position repeats identically across words.
   The full 294,912-bit table is derived from the probe structure
   and then verified by sampling: at least one fresh probe word/bit
   position not in the probe set, predicted from the derived table
   and checked against a new Vivado run, before the table is trusted.
3. Pass C: the `.ll` file gives the frame range of every URAM
   column; that lands in the device file as the URAM column mapping.
4. Cross-check each differing bit against the variant's `.ll` entry
   for the URAM cell.
5. Emit, per attribute value and per content bit position, the list
   of (minor, frame_ofst) pairs plus the value-to-bit mapping, in the
   same schema as the existing `LutLoc` tables.

## Where the data gets vendored

The derived encoding lands under
`crates/helion-ultrascale/data/` as `segbits_uram.db` (prjuray-style
segbits lines, one per attribute value) and/or a `URAM` section in
the BitFiltrator-style tables, with provenance (source, Vivado
version, part string, run manifest) recorded in
`crates/helion-ultrascale/data/PROVENANCE.md`, the single provenance
index. The raw `.bit` / `.rbt` / `.ll` files are working data, not
vendored: only the derived encoding plus the run manifest are.
Once vendored, the `UramBlocked` fail-loud paths in
`crates/helion-ultrascale/src/bitgen.rs`, `src/uram.rs`, and
`src/bitloc.rs` are replaced by real emission, with regression tests
and the URAM golden design diffed to zero.

## Relationship to the earlier phase scaffold on this branch

This package does not replace
`docs/ultra-bitgen-crew/vivado-fuzzing/job5-uram/` (commit 013044f2,
same branch). That directory is the phase scaffold: `tcl/uram.v`
(the UG974-defaults template, G_SIZE generate), `tcl/
one_uram_in_every_uram_column.tcl` (SLR- and clock-region-aware
phase-1 runner), and its README (phase plan, package defects). This
package is the runnable job built on it: the driver, the table-driven
generators, and passes A, B, and C. Where the two disagree, the
difference is recorded here, not hidden:

- Variant values: this package's table agrees with the scaffold's
  phase-2 table on every value except SELF_MASK (here `11'h7fe`, one
  mask bit cleared, so the diff stays attributable; there
  `11'h000`) and AVG_CONS_INACTIVE_CYCLES (here 100, there 100000).
  The scaffold values are follow-up runs, not primary runs.
  CASCADE_ORDER has three variants per port in both (FIRST, MIDDLE,
  LAST; 38 variants total), after this package was extended from
  FIRST-only to match.
- Port ties: this package's generator ties BWE to 0 and RDB_WR to 1,
  matching `uram.v`, so the pass-A baseline is the same design as
  the scaffold template with G_SIZE=1. The RDB_WR polarity is not
  asserted in either document; the tie is constant across all
  variants in both, so no sweep result depends on it.
- Pass C vs phase 1: the scaffold's phase-1 script enumerates the
  bottom-most site per URAM column SLR by SLR and clock region by
  clock region, using the shipped `helpers.tcl`, with
  `-no_timing_driven` synthesis, no `opt_design`, and
  `BITSTREAM.GENERAL.COMPRESS FALSE` plus `CRC DISABLE` (deliberate
  there, for deterministic frames). This package's pass C enumerates
  the lowest site per X (the same site: dictionary order is X then
  Y, so lowest per X is bottom-most per column) and runs the
  standard flow template with `opt_design` and neither CRC nor
  compression override, like passes A and B. The two pass-C forms
  are alternatives, not both to run: run this package's pass C,
  which the driver drives. Two open items for the crew before
  diffing any phase-1 output against the Job 1 empty design: the
  COMPRESS/CRC settings must match the Job 1 empty run's settings
  or the framing bits will differ for reasons that have nothing to
  do with URAM, and the `-no_timing_driven` / skipped `opt_design`
  choice must be one convention, not two.
- Phase 3 vs pass B: the scaffold's phase 3 is probe-first (inspect
  the phase-1 `.ll`, run `report_property` on a placed URAM cell,
  design the sweep only if a contents mechanism is found). Pass B
  supersedes it with a sourced mechanism: the XPM `.mem` file
  (UG974 XPM_MEMORY_SPRAM). If the pass-B control fails (p012 .rbt
  identical to p000 .rbt), the scaffold's `report_property` step is
  the fallback: run it on the placed cell, record the full property
  list in `runs.txt`, and report the content encoding as still
  blocked with that list attached.

The shipped package (`~/workspace/your_files/vivado-fuzzing-package.zip`,
`instructions.md` identical to
`~/workspace/your_files/vivado-fuzzing-instructions.md`) had these
gaps for job 5, all fixed on this branch:

- F1 (missing): the shipped package had no job-5 script at all.
  The instructions described the method in one paragraph ("Mirror
  the BRAM approach (one URAM per column, then content sweep)"). The
  branch's phase scaffold
  (`docs/ultra-bitgen-crew/vivado-fuzzing/job5-uram/`, commit
  013044f2) added the template, the phase-1 runner, and the phase
  plan. This directory adds the runnable job on top of it: the
  table, both generators, the site pickers, the three per-run
  drivers, the column design generator, and the job driver.
- F2 (wrong method): mirroring the BRAM approach literally does not
  work for URAM288, because URAM288 has no INIT attributes to sweep
  (UG974 lists 34 attributes, no INIT; see
  `crates/helion-ultrascale/data/URAM288_SEARCH.md`). Pass B uses
  the XPM `.mem` initialization file instead, which is the documented
  content path for this primitive.
- F3 (wrong): the package's only DSP script (the model for the
  per-run drivers) writes the bitstream WITHOUT `-raw_bitfile` and
  WITHOUT `-logic_location_file`, so it produces neither the `.rbt`
  the diff method needs nor the `.ll` that maps the cell to its
  frames. All three job-5 drivers request both on every run, per the
  instructions ("ALWAYS request both on every run").
- F4 (divergent): the same script disables CRC
  (`BITSTREAM.GENERAL.CRC DISABLE`) and compression, diverging from
  the instructions' standard flow template, which sets neither. The
  job-5 drivers set neither, so the only difference between two runs
  is the fuzzed attribute or content.
- F5 (unpinned): the instructions say "one URAM per column, then
  content sweep" with no attribute list, no probe set, and no
  control. The table pins the exact 34 attributes and 38 variant
  values, the probe generator pins the exact 13 probes including the
  all-ones control, and pass C pins the column enumeration.

## Scope notes

- Dynamic control ports (`EN_A`, `EN_B`, `RDB_WR_A`, `RDB_WR_B`,
  `RST_A`, `RST_B`, `CLK`) are live inputs, not primitive
  attributes. They have no sweep entry: their values are routing,
  not URAM config bits. Job 5 fuzzes the 34 primitive attributes
  only.
- `SELF_ADDR_A/B` and `SELF_MASK_A/B` are 11-bit values. The variant
  values (`11'h001`, `11'h7fe`) are minimal single-bit changes, so
  the diff stays attributable to the attribute.
- `AUTO_SLEEP_LATENCY` (8 to 3) and `AVG_CONS_INACTIVE_CYCLES` (10
  to 100) are the only integer attributes; both keep their kind as
  `int` in the table.
- Do not modify a variant Verilog or a probe `.mem` between runs.
  The generator output is the input to every run for that id.
- ECC and initialization are mutually exclusive in the XPM macro
  (UG974: "When ECC is enabled ... Initialization [is] not
  supported"), so pass B runs with `ECC_MODE` "no_ecc". The
  `EN_ECC_*` attribute encodings come from pass A, not pass B.

## Interface to the synthesis-mapping worker

The sibling worker on `fm/ultra-bitgen-synthmap` owns synthesis
mapping to UltraScale primitives, including URAM288. The interface
between that work and this block is:

- Vocabulary: the mapper must use exactly the 34 attribute names in
  the job-5 table (`uram_variant_table.tsv`, same list as
  `gen_uram_variants.py` `EXPECTED_ATTRIBUTES` and
  `crates/helion-ultrascale/src/uram.rs` `URAM288_ATTRIBUTES` plus
  the two excluded `CASCADE_ORDER_*` names, taken from UG974). Job 5
  fuzzes exactly that set. Any attribute the mapper emits that job 5
  did not fuzz stays blocked. Note the count differs from
  `URAM288_ATTRIBUTES` in `src/uram.rs` on purpose: that list has 32
  names, the bare-name detector subset. `CASCADE_ORDER_A` and
  `CASCADE_ORDER_B` are excluded there because they are also
  RAMB36E2 attribute names, and a bare feature of that name on a
  non-URAM tile must stay `UnknownFeature`; they are still fuzzed by
  job 5, and they still block whenever the tile or feature itself
  names URAM288 or a URAM tile.
- FASM: the mapper emits URAM288 cells as FASM entries naming a
  URAM288 site/tile and its attributes. The emitter is the
  enforcement point: until the job-5 encoding lands, any such entry
  fails with `BitgenError::UramBlocked`, which names the missing
  encoding and the unblocker.
- No workaround: the mapper must not invent URAM bits, and must not
  silently drop a URAM288 cell (or its attributes) to make a design
  emit. A design that needs a URAM either maps it and blocks loudly
  at bitgen, or is out of scope for that run. Dropping a cell to
  force emission would be a miscompile.
- Coordination goes through the orchestrator only. Neither worker
  edits the other's branch.

## Rules

- Never invent numbers. If a run fails, report the failure with the
  log. A zero diff is a result, not a failure.
- Every bitstream must come from a real Vivado run on the cluster.
- Do not modify the test designs between baseline and variant runs
  except for the single attribute being fuzzed (pass A) or the single
  `.mem` content being probed (pass B). Placement is fixed to one
  site for all runs in passes A and B.
- Keep the Vivado version consistent across all runs in the job.
- If the exact part string differs from `xcvu3p-ffvc1517-2-e`, use
  what `get_parts` reports and note it in `runs.txt`.
- Never claim Vivado-equivalence without a real Vivado diff at zero.
