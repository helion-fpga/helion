# ZB-11: IOB output driver (test t01, no clock involved)

- Track: ZedBoard, backend. Investigator findings H1, H3, H6 touch this
  test.
- Branch: none (test written, never run on hardware). Worktree:
  ~/workspace/helion-zed-bringup.
- Status: test written, hardware unproven.

## Goal

Prove the IOB output path works in isolation, with no clock, no FFs, and
no CARRY4 involved, so that any later failure in the clocked tests can
be blamed on the clock path and not on the outputs.

## Done when

Test t01 passes on the board: LD0, LD1, LD2, LD3, LD4 brightly lit, LD5
dark. Any of LD0-LD4 dark, or LD5 lit, is a failure.

## What is already done

- The test is written: ~/workspace/zedboard-hw-loop/tests/t01_iob_static.sv,
  with pass/fail criteria in test_manifest.md. It isolates the OBUF / IOB
  output config. No clock, no FFs.
- Pin map is verified: LD0 T22, LD1 T21, LD2 U22, LD3 U21, LD4 V22,
  LD5 W22 (examples/zedboard.xdc).
- The investigator flagged three things that touch this test:
  - H1 (route.rs:140-145): the router comment says "The output path must
    go through the OLOGIC bypass", but emit.rs emits no OLOGIC
    configuration. If the OLOGIC needs explicit bypass configuration
    bits, they are missing.
  - H3 (pack.rs:275-315): the IBUF PAD connection is implicit. There is
    no explicit connection from the top-level port to the IBUF's PAD pin
    in the netlist; the router sees the fabric-side net but the pin-to-
    IBUF path is assumed, not modeled.
  - H6 (emit.rs:96-105): LUT input tie-off is not verified. Unconnected
    inputs are "tied low at emission" per the pack.rs comment, but emit.rs
    never verifies the assumption. If synth assumes differently, the LUT
    function is wrong.

## What was tried and what failed

- Nothing hardware-tried. t01 has never been flashed.
- H1 answered by ZB-10 diff (Nov 8, 2026, Mac), no hardware needed:
  the output path does NOT use an OLOGIC bypass. Vivado configures
  the OLOGIC explicitly (OQUSED + OMUX.D1 + DATA_RATE_TQ.BUF), now
  implemented in emit.rs. The old "OLOGIC bypass" comment was wrong.

## What to do

1. Run t01 first, before the clocked tests. It is the cheapest possible
   hardware signal: if the outputs do not work with no clock involved,
   the problem is in the IOB path, not the clock.
2. If t01 fails, work the three findings in order:
   a. H1: check the X-Ray DB for OLOGIC bypass bits and confirm whether
      the output path needs them. Compare against the Vivado bitstream
      (ZB-10) for the same outputs.
   b. H3: make the PAD connection explicit in the netlist if the failure
      points at the IBUF path.
   c. H6: verify the LUT INIT against the unconnected-input assumption;
      if synth and emit disagree, fix the assumption in one place and
      record which.
3. If t01 passes, the IOB path is proven and the clocked tests (t02-t05)
   can be read as pure clock/control tests.

## Files and commands

- Test: ~/workspace/zedboard-hw-loop/tests/t01_iob_static.sv,
  test_manifest.md (t01 section).
- Code: crates/helion-xilinx7/src/route.rs (lines 140-145, OLOGIC),
  src/pack.rs (lines 275-315, IBUF PAD), src/emit.rs (lines 96-105, LUT
  INIT).
- Pin map: examples/zedboard.xdc.

## Dependencies

Depends on ZB-00 (Mac access) and ZB-01 (the flash loop). Unblocks the
interpretation of t02-t05: if t01 passes and t02 fails, the clock is the
problem, not the outputs.

## Rules

- No em dashes anywhere.
- Run t01 before the clocked tests. Do not skip the cheap signal.
