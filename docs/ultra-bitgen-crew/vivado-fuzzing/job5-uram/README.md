# Job 5: URAM288 fuzzing (UltraScale+ bitgen)

Status: READY TO RUN, blocked only on a working Vivado license. The
job scripts in this directory did not exist in the fuzzing package:
Job 5 was a three-line DEFERRED paragraph in
`vivado-fuzzing-instructions.md` with no `.tcl` and no `.v` file.
This directory supplies the missing job. Nothing here has been run:
there is no Vivado on this VM, the license server state is UNKNOWN,
and no run is claimed.

## What is missing, and the exact evidence

No URAM288 content or config encoding exists in any public source.
Verified 2026-11-19 on branch `fm/ultra-bitgen-uram`:

- prjuray-db reference clone, commit
  `affbc5e555ebae16475f32e8fb2d6565d4204f3f` (304 files):
  `grep -rni "uram" ~/workspace/prjuray-db-ref` returns nothing
  (exit 1). No URAM segbits file, no URAM tile type, no URAM mention
  anywhere. Its `zynqusp/` directory has `segbits_bram.*` and
  `segbits_rclk_*` files but nothing for URAM.
- BitFiltrator `ULTRASCALE_PLUS.json`, SHA-256
  `c6320fe7322d918fc3995f9122f62b0f4ab7b282b517bf8231d0c63ac70b436c`
  (vendored copy and reference clone are byte-identical): top-level
  keys are only `BRAM`, `CLEL_L`, `CLEL_R`, `CLEM`, `CLEM_R`; the
  whole 4,670,414-byte file contains zero occurrences of `URAM`.
  The older `ULTRASCALE.json` also contains zero.
- Vendored segbits DBs (`crates/helion-ultrascale/data/segbits_*.db`):
  no line mentions URAM. The only URAM data anywhere in the repo is
  tile-to-site names in the device files
  (`["URAM_URAM_FT","URAM288"]`, `["URAM_URAM_DELAY_FT","URAM288"]`),
  which name the tile and site but carry no bit locations, no INIT
  table, and no config bits. `PROVENANCE.md` already records this:
  "URAM288: MISSING in both public sources. Fail loud until it lands."

The exact missing data:

1. URAM column FAR mapping: which configuration frames belong to
   each URAM column (block type, row major, column major per URAM
   tile). The device file resolves SLICE, INT, and BRAM tiles only;
   a URAM tile name resolves to nothing today.
2. URAM288 config-bit encoding: which frame bits each of the 34
   URAM288 attributes (UG974 list below) controls.
3. URAM288 content encoding: which frame bits hold the 4096 x 72
   initial memory contents, and how Vivado expresses them at all
   (URAM288 has no INIT attributes, see phase 3).

Unblocker: a working Vivado license plus this job's runs. Nothing
else unblocks it.

## How to run (on the cluster, when the license works)

Prerequisites and batch-mode rules are the package instructions
(`vivado-fuzzing-instructions.md`): SSH to sc.rc.usf.edu as
srivastava1, batch mode only, non-project flow, keep the Vivado
version consistent across all runs in the job, confirm the exact
part string with `get_parts` (expect `xcvu3p-ffvc1517-2-e`).

Setup: copy `tcl/one_uram_in_every_uram_column.tcl` and `tcl/uram.v`
into `vivado-fuzz-pkg/tcl/` next to `helpers.tcl`. `helpers.tcl` is
reused verbatim from the package and is not duplicated here.

Gate: do not start until a real Vivado checkout is verified
possible. A license port answering is not a checkout. If the
checkout fails, stop and report the failure with the log; do not
silently skip runs.

## Phase 1: one URAM per column (1 run, HIGHEST PRIORITY)

Run:

```
vivado -mode batch -source one_uram_in_every_uram_column.tcl -notrace \
  -tclargs xcvu3p-ffvc1517-2-e job5_phase1.bit job5_phase1_urams.txt
```

What it does: enumerates the bottom-most URAM288 site in every URAM
column (SLR by SLR, clock region by clock region, row-major order),
instantiates one `URAM288` per column from `uram.v` (all attributes
at UG974 defaults), places each instance at its site, routes, and
writes the bitstream with `-raw_bitfile -logic_location_file`.

What to bring back: `job5_phase1.bit`, `job5_phase1.rbt`,
`job5_phase1.ll`, `job5_phase1.dcp`, `job5_phase1_urams.txt` (the
ordered site list), plus `runs.txt` (Vivado version, exact part
string, whether the run completed cleanly).

What it unblocks: the URAM column FAR mapping for the device file.
Job 1's four runs (empty, FF, BRAM, DSP) do not touch any URAM
column, so URAM FARs come from this run, not from Job 1. The Helion
crew diffs the phase-1 `.rbt` against the Job 1 empty-design `.rbt`
to find each URAM column's frames, using the `.ll` file as ground
truth for which frame/bit each placed URAM maps to.

## Phase 2: attribute sweep (38 variant runs + 1 baseline)

Baseline: one URAM288 at the phase-1 site list is not needed; use a
single-URAM design instead. Instantiate `uram.v` with `G_SIZE=1`,
place the instance at the first site from `job5_phase1_urams.txt`,
run the standard flow, save `job5_phase2_baseline.bit/.rbt/.ll`.
Then, for each attribute below, change only that attribute in
`uram.v` to the listed non-default value, re-run, and save the
variant `.rbt` plus a `runs.txt` line naming the attribute and value.
Diff each variant `.rbt` against the baseline `.rbt`; the differing
bits are that attribute's encoding. Do not modify anything else
between baseline and variant runs.

Attribute list (AMD UG974, URAM288 page, 2026.1 English; defaults
in parentheses). 34 attributes, 38 variant runs:

| Attribute | Default | Variant run(s) |
|---|---|---|
| AUTO_SLEEP_LATENCY | 8 | 3 |
| AVG_CONS_INACTIVE_CYCLES | 10 | 100000 (power-estimation only; a zero diff is data, not failure) |
| BWE_MODE_A | PARITY_INTERLEAVED | PARITY_INDEPENDENT |
| BWE_MODE_B | PARITY_INTERLEAVED | PARITY_INDEPENDENT |
| CASCADE_ORDER_A | NONE | FIRST, MIDDLE, LAST |
| CASCADE_ORDER_B | NONE | FIRST, MIDDLE, LAST |
| EN_AUTO_SLEEP_MODE | FALSE | TRUE |
| EN_ECC_RD_A | FALSE | TRUE |
| EN_ECC_RD_B | FALSE | TRUE |
| EN_ECC_WR_A | FALSE | TRUE |
| EN_ECC_WR_B | FALSE | TRUE |
| IREG_PRE_A | FALSE | TRUE |
| IREG_PRE_B | FALSE | TRUE |
| IS_CLK_INVERTED | 1'b0 | 1'b1 |
| IS_EN_A_INVERTED | 1'b0 | 1'b1 |
| IS_EN_B_INVERTED | 1'b0 | 1'b1 |
| IS_RDB_WR_A_INVERTED | 1'b0 | 1'b1 |
| IS_RDB_WR_B_INVERTED | 1'b0 | 1'b1 |
| IS_RST_A_INVERTED | 1'b0 | 1'b1 |
| IS_RST_B_INVERTED | 1'b0 | 1'b1 |
| OREG_A | FALSE | TRUE |
| OREG_B | FALSE | TRUE |
| OREG_ECC_A | FALSE | TRUE |
| OREG_ECC_B | FALSE | TRUE |
| REG_CAS_A | FALSE | TRUE |
| REG_CAS_B | FALSE | TRUE |
| RST_MODE_A | SYNC | ASYNC |
| RST_MODE_B | SYNC | ASYNC |
| SELF_ADDR_A | 11'h000 | 11'h001 |
| SELF_ADDR_B | 11'h000 | 11'h001 |
| SELF_MASK_A | 11'h7ff | 11'h000 |
| SELF_MASK_B | 11'h7ff | 11'h000 |
| USE_EXT_CE_A | FALSE | TRUE |
| USE_EXT_CE_B | FALSE | TRUE |

Expected analysis note for the crew: DSP config bits live in
CLB_IO_CLK frames, not a separate block type (instructions, Job 2).
Do not assume URAM config bits live in a URAM block type either;
scope the diff to the URAM column's frames using the phase-1 FAR
map and the `.ll` file, and let the diff say where the bits are.

## Phase 3: content encoding (PROBE FIRST, sweep second)

URAM288 has no INIT attributes in UG974 (unlike RAMB18E2, which has
INIT_00..INIT_3F and INITP_00..INITP_07). The BRAM content sweep
does not transfer: there is nothing in `uram.v` to vary that would
set initial contents. Do not invent an INIT spelling.

Probe steps, in order, before any sweep:

1. Inspect the phase-1 `.ll` file: it maps each placed URAM to its
   frame and bit positions. Record which frames the URAM cells touch.
2. In Vivado, on the phase-1 placed design, run `report_property`
   on one URAM cell and record every property that names initial
   contents or memory initialization. Record the full property list
   in `runs.txt` even if nothing relevant appears.
3. If a contents mechanism is found (for example a cell property or
   a `read_memdata`-style flow), design the sweep then: one variant
   run per contents change (single-bit flips first, then one full
   word), diffing `.rbt` against the phase-1 baseline. If no
   mechanism is found, report that as the phase-3 result with the
   property list attached; the content encoding stays blocked and
   the report, not a guess, is the deliverable.

## What the job must produce, and where it gets vendored

Per run: `.bit`, `.rbt`, `.ll`, `.dcp`, plus the `runs.txt` log
(design, attribute changed, Vivado version, exact part string,
whether the run completed cleanly) and the Tcl scripts actually used.

Vendoring target (Helion crew, after the runs land):

- `crates/helion-ultrascale/data/segbits_uram.db`: config-bit
  encodings in the prjuray segbits format the `FileSegbitsDb`
  loader already reads (tile type, feature, frame/bit list).
- `crates/helion-ultrascale/data/ultrascale_plus_lut_ff_bram.json`
  (or a new `ultrascale_plus_uram.json` if the table shape differs):
  content-bit locations per URAM site, same
  `(minor, frame_ofst)` form as the BRAM tables.
- `crates/helion-ultrascale/data/PROVENANCE.md`: one entry per
  vendored file (source: Vivado-generated fuzzing output, Vivado
  version, part string, scripts used, run dates). Generated data
  carries no third-party license; provenance records method and
  version instead.
- The URAM column FAR mapping goes into the device file
  (`data/xcvu3p.json` / `data/xcku3p.json` URAM column majors),
  replacing the current state where no URAM tile resolves.

## Package defects found while preparing this job

These are defects in the shipped package
(`~/workspace/your_files/vivado-fuzzing-package.zip`, instructions
identical to `~/workspace/your_files/vivado-fuzzing-instructions.md`,
verified with `diff` 2026-11-19). They are recorded here, not fixed
in the package itself: this branch owns only the URAM job, and the
DSP/BRAM scripts belong to their own blocks.

1. Job 5 had no scripts at all. Fixed by this directory.
2. `one_dsp_in_every_dsp_column.tcl` calls plain `write_bitstream
   -force <out>`: no `-raw_bitfile`, no `-logic_location_file`.
   The instructions say ALWAYS request both on every run. The DSP
   per-column run therefore brings back neither the `.rbt` (the diff
   input) nor the `.ll` (the ground truth). Same gap, partial, in
   `one_bram_in_every_bram_column.tcl` and
   `all_brams_in_one_bram_column.tcl`: they request
   `-logic_location_file` but not `-raw_bitfile`, so no `.rbt`.
   The URAM script in this directory requests both.
3. Job 1's `dump_device.tcl` snippet writes one INT tile to
   `tiles.txt` and then `break`s out of the loop. It is vestigial;
   the text below it already says to bring back the full
   `report_property` of the part instead. Harmless, but the snippet
   should not be run as written.
4. Job 1's four runs do not cover URAM columns. Any plan that treats
   Job 1 as completing the device file is wrong for URAM: the URAM
   column FAR mapping waits on Job 5 phase 1.

5. `get_cells` with an unescaped generate index is a Tcl error, and
   this job's script copied it from the DSP script. The shipped
   `one_dsp_in_every_dsp_column.tcl:153` writes
   `get_cells "DSP48E2_gen[${idx}].DSP48E2_inst"`, and the same
   unescaped pattern is in `one_bram_in_every_bram_column.tcl:143`,
   `one_ff_in_every_clb_column.tcl:132`, and the `all_*` scripts, all
   byte-identical to the BitFiltrator reference clone (commit
   `79b9bd66`, verified with `diff`). Inside double quotes `[${idx}]`
   is Tcl command substitution: the index is evaluated as a command
   name. Proven with stock `tclsh` 2026-11-19:
   `puts "DSP48E2_gen[${idx}].DSP48E2_inst"` fails with
   `invalid command name "3"`, while the escaped form
   `"DSP48E2_gen\[${idx}\].DSP48E2_inst"` yields
   `DSP48E2_gen[3].DSP48E2_inst`. The URAM script in this directory
   is fixed (escaped brackets, with a comment at the line so nobody
   removes them). The shipped DSP/BRAM/FF/LUT scripts are NOT fixed
   here: they belong to their own blocks, and the fix is one
   backslash pair per `get_cells` line. Flagged to the orchestrator
   as package-wide: every per-column script that places cells by
   generate index needs the same fix before any of them runs.

## Rules

- Never invent numbers. If a run fails, report the failure with the
  log.
- Every bitstream must come from a real Vivado run on the cluster.
- Do not modify the test designs between baseline and variant runs
  except for the single attribute being fuzzed.
- Keep the Vivado version consistent across all runs in a job.
- If the exact part string differs from `xcvu3p-ffvc1517-2-e`, use
  what `get_parts` reports and note it in `runs.txt`.
