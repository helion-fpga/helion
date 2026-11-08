# ZB-02: BUFGCTRL I0 configuration (clock passes to the flip-flops)

- Track: ZedBoard, backend. Investigator finding C1.
- Branch: fm/zedboard-fix-bufg, tip 9b1c15f. Worktree:
  ~/workspace/helion-zed-bringup.
- Status: code done, hardware unproven. Validated by ZB-01.

## Goal

The BUFGCTRL on the clock path must be configured so the 100 MHz clock
actually reaches the flip-flops, with the correct input selected.

## Done when

Test t02 (t02_bufg_toggle) passes on the board: LD0 glows DIM (a 50 MHz
toggle at 50 percent duty), LD1-LD7 dark. That proves the clock reaches
the FF. LD0 fully dark means the clock never reached the FF.

## What is already done

- The original bug: emit.rs emitted only 2 of the required BUFGCTRL FASM
  features (IN_USE plus IS_IGNORE0_INVERTED). The clock was marked used
  but never configured to pass.
- First fix attempt was wrong: it emitted all 10 features from the X-Ray
  DB (IN_USE, INIT_OUT, IS_IGNORE0_INVERTED, IS_IGNORE1_INVERTED,
  PRESELECT_I1, ZINV_CE0, ZINV_CE1, ZINV_S0, ZINV_S1, ZPRESELECT_I0).
  That build (v2) still went dark on the board. The mistake: emitting
  ZINV_S1, ZINV_CE1, PRESELECT_I1 selects the I1 input instead of I0.
- Correct fix, commit e0c5ce0 on fm/zedboard-fix-bufg: the I0 to O config
  is 7 features (IN_USE, INIT_OUT, IS_IGNORE0_INVERTED,
  IS_IGNORE1_INVERTED, ZINV_S0, ZINV_CE0, ZPRESELECT_I0). S1=0 and CE1=0
  by absence.
- Verification commit 9b1c15f: control-signal FASM resolves to documented
  X-Ray DB bits. E2E test passed (502 s).
- The ZINV_ bit polarities were a guess in the original code and were never
  validated against hardware or docs. The investigator report (C1) says:
  either the polarity is wrong, or additional routing bits are needed, or
  the wrong BUFG site is selected.

## What was tried and what failed

- 2-feature emit: dark. 10-feature emit (v2): dark. Both programmed
  fine (done=1). The 10-bit version was wrong in a specific way: it
  selected I1 instead of I0, so the clock input the design drives was not
  the one the buffer passed.

## What to do

1. Wait for ZB-01: if v3 blinks, the 7-feature config plus the tie-offs
   (ZB-01) already prove the clock path end to end, and this sub-problem
   reduces to running t02 as the isolated proof.
2. If v3 stays dark, this is the first suspect to work. In order:
   a. Read the X-Ray DB segbits_clk_bufg_bot_r.db and confirm every one of
      the 7 features against the DB, bit by bit, and confirm no other
      feature is needed for I0 to O.
   b. Compare the emitted FASM for the BUFGCTRL tile against a Vivado
      bitstream for the same clock path (see ZB-10). Any differing bit is
      a candidate fix.
   c. If the DB and Vivado agree with the 7 features and the board is
      still dark, check the BUFG site selection: is the right BUFG site
      chosen for the Y9 clock pin (see ZB-07, site allocators)?
   d. Only as a last resort, try the inverted ZINV polarity as a
      hardware experiment, one bit at a time, with a photo after each
      flash. Record exactly which polarity was tried and the result.
3. Run t02 and report the exact observation (LD0 dim, dark, or bright).

## Files and commands

- Code: crates/helion-xilinx7/src/emit.rs, BUFG emit loop (investigator
  cites lines 163-178 for the original 10-feature version).
- DB: ~/workspace/xilinx-dbsrc, segbits_clk_bufg_bot_r.db.
- Test: ~/workspace/zedboard-hw-loop/tests/t02_bufg_toggle.sv,
  test_manifest.md (t02 section).
- Pin map: examples/zedboard.xdc (clk Y9, LD0 T22).

## Dependencies

Depends on ZB-01 for hardware validation. Related to ZB-07 (site
selection) and ZB-10 (Vivado diff as ground truth).

## Rules

- No em dashes anywhere.
- Never fabricate. The ZINV polarity is a guess until hardware proves it;
  say so plainly until t02 passes.
- One bit change at a time on hardware experiments, with a photo after
  each flash.
