# US-03: DSP48E2, URAM288, and CLK encodings

- Track: UltraScale+, bitgen.
- Branch: fm/ultra-bitstream. Worktree:
  ~/workspace/helion-ultra-bitstream.
- Status: BLOCKED. No encodings exist in any public source. Needs Vivado
  fuzzing, which needs the license.

## Goal

Encode DSP slices, URAM, and clocking configuration bits so designs that
use them can diff to zero.

## Done when

DSP48E2, URAM288, and CLK each read green on a golden design that uses
them, with no other block regressed.

## What is already done

- Nothing for these blocks. The v1 goldens do not exercise them, so they
  are blocked, not red. The reference data facts are established: the
  prjuray-db reference clone at ~/workspace/prjuray-db-ref (CC0 1.0) has
  no UltraScale+ reference.bit files (7-series only); BitFiltrator's
  ULTRASCALE_PLUS.json documents INT PIPs and some CLB/BRAM/CMT/HPIO/HDIO
  segbits, but no DSP48E2 bits and no URAM288.
- The fuzzing package exists: ~/workspace/your_files/vivado-fuzzing-
  instructions.md plus vivado-fuzzing-package.zip (30 files). Job 2 is
  DSP48E2 bits; jobs 4 and 5 are deferred. The package was delivered, the
  jobs were never run.
- The out-of-scope rules are set (INTERFACES.md): DSP48E2 config bits are
  fail-loud until segbits-vendor lands the encoding; URAM content is
  fail-loud until the device file lands; MMCM attribute fuzzing is out of
  scope until a design needs it (tie safe default, fail loud).

## What was tried and what failed

- Nothing tried. No fuzzing run has ever happened for these blocks; the
  license server refused connections from Oct 22 through Nov 7, 2026, and
  appeared to answer on Nov 8 at about 10:30 EST, but that finding was
  proven void the same day (see docs/HANDOFF.md), and a real checkout is
  still unverified.

## What to do

1. Check the license first: the driver checks it every run. Do not start
   fuzzing until a real Vivado checkout is verified possible (a port
   answering is not a checkout; no lmutil or Vivado exists on this VM).
2. When the license works, run the fuzzing jobs from the package: job 2
   (DSP48E2 bits) first, then the URAM and CLK jobs. Each job produces
   segbits that must be vendored under crates/helion-ultrascale/data/
   with provenance recorded in PROVENANCE.md (source, license, commit).
3. Build a golden design for each block (same pattern as the blinky and
   counter goldens) and diff it to zero.
4. Until the encodings land, keep the fail-loud behaviour: a design using
   DSP, URAM, or clocking features without an encoding must error
   clearly, never emit guessed bits.

## Files and commands

- Fuzzing package: ~/workspace/your_files/vivado-fuzzing-instructions.md,
  vivado-fuzzing-package.zip.
- Contracts: docs/ultra-bitgen-crew/INTERFACES.md (out-of-scope rules,
  SegbitsDb, PROVENANCE.md as the single provenance index).
- Code: crates/helion-ultrascale/src/ (segbits.rs, device_file.rs).
- Related: ZB-06 (ZedBoard DSP fail-loud is the same rule at 7-series
  scale).

## Dependencies

Blocked on the Vivado license (US-08 is the same blocker from the other
side). Unblocks US-07 (the random generator cannot cover DSP/URAM/CLK
until their encodings exist).

## Rules

- No em dashes anywhere.
- Guessed values are never emitted. Fail loud on missing encodings.
- Vendor with provenance. PROVENANCE.md is the single provenance index.
- Never claim Vivado-equivalence without a real Vivado diff at zero.
