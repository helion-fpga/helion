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

## Vivado ground truth (Nov 8, 2026, Mac, from ZB-10)

First same-RTL diff ever run: Helion examples/blinky_reg.sv build
(/tmp/x7/blinky_reg_helion.bit) vs Vivado GT-blinky.bit. Caution: the
diffed Helion build is from fm/zedboard-fix-router, which does NOT
contain the 7-feature fix (that lives on fm/zedboard-fix-bufg). What
the diff proves about this branch's BUFG emit:

- Helion sets ZINV_CE1 and ZINV_S1 (frame 0x42109a word 93 bit 11,
  ours=1 ref=0). Vivado does not. Confirms the I1-path diagnosis: do
  not emit these for the I0 clock.
- Vivado sets IS_IGNORE1_INVERTED (frame 0x42109b word 93 bit 12,
  ours=0 ref=1). Helion does not. Add it.
- Vivado sets CLK_BUFG input-mux pips CK_MUXED0, IMUX24_0, IMUX28_0
  (frame 0x42109a word 93 bit 8, ours=0 ref=1). Helion routes no
  input mux into the BUFG. Missing clock-input routing.
- Helion sets ZPRESELECT_I0 (frame 0x42109a word 93 bit 2, ours=1
  ref=0). Vivado does not. Open question against the 7-feature list:
  recheck whether I0 to O needs it.
- IS_IGNORE0_INVERTED does not differ: both sides agree on it.
- Wider clock tree: Vivado enables HCLK region buffers (word-50 bits
  across HCLK_R, HCLK_IOI3, CLK_HROW_BOT_R, all ref=1 ours=0).
  Helion emits zero HCLK bits. Separate gap, file it if it is not
  owned already.

## Second run (Nov 8, 2026, Mac, after the emit fix)

emit.rs now emits exactly IN_USE, ZINV_CE0, ZINV_S0,
IS_IGNORE1_INVERTED (commit on fm/zedboard-fix-router; pinned by
unit test emit_bufg_i0_config_matches_vivado). Rebuilt blinky_reg
and re-ran the ZB-10 diff: 4,741 down to 4,737, and the BUFGCTRL
config block is zero differing bits. Consequences:

- ZPRESELECT_I0 question answered: removed, block is zero without
  it.
- The 7-feature list on fm/zedboard-fix-bufg (which adds INIT_OUT,
  IS_IGNORE0_INVERTED, ZPRESELECT_I0) sets 3 bits Vivado does not
  set. For Vivado-identical output those must go; whether they are
  harmless on hardware is unproven until t02.
- Still open on the clock path (router work, not static emit):
  CLK_BUFG input-mux pips, the BUFGCTRL0_O to GCLK0 output pip,
  and the REBUF enables. Helion routes the clock input through a
  GFAN/GCLK detour instead.

## Rules

- No em dashes anywhere.
- Never fabricate. The ZINV polarity is a guess until hardware proves it;
  say so plainly until t02 passes.
- One bit change at a time on hardware experiments, with a photo after
  each flash.
