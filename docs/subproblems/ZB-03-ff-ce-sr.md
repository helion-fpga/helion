# ZB-03: Flip-flop clock-enable and set/reset

- Track: ZedBoard, backend. Investigator finding C3.
- Branch: fm/zedboard-fix-ffctrl, tip d2d067a (plus verification commit
  9b1c15f on fm/zedboard-fix-bufg). Worktree:
  ~/workspace/helion-zed-bringup.
- Status: code done, hardware unproven. Validated by tests t04 and t05.

## Goal

Flip-flops must honor their clock-enable (CE) and set/reset (SR) pins
instead of silently ignoring them.

## Done when

Test t04 passes (CE: LD0 behaviour changes when BTNC is pressed and
released) and test t05 passes (SR: LD0 resets when BTNC is pressed),
both on the board. The exact pass criteria are in test_manifest.md.

## What is already done

- The original bug (investigator C3): emit.rs emitted ZINI and FFMUX for
  each FF but never ZRST, CEUSEDMUX, or SRUSEDMUX. The pack stage had the
  data all along: PackedFf in pack.rs has `ce_net: Option<String>` and
  `sr_net: Option<String>`, captured correctly from the IR at pack.rs
  lines 236-240. emit.rs simply never emitted them. Any FF with CE or SR
  silently ignored those pins.
- Fix, commit b2b4768 on fm/zedboard-fix-ffctrl: ZRST emitted per FF when
  sr_net is present; CEUSEDMUX / SRUSEDMUX emitted per slice,
  deduplicated. 5 emit tests pass.
- Verification commit 9b1c15f: control-signal FASM resolves to documented
  X-Ray DB bits.

## What was tried and what failed

- Nothing hardware-tried yet. The fix is code-complete and unit-tested
  only. No Helion bitstream with CE/SR exercised has ever been flashed.

## What is still missing (the actual work)

1. FFSYNC is not implemented. Find where the X-Ray DB defines the FF sync
   vs async select, and emit it. Until then, any design relying on a
   specific sync behaviour is unproven.
2. SET vs RESET is hardcoded to RESET. Find the DB feature that selects
   SET, and emit it from the IR's actual set/reset attribute instead of
   the hardcoded value.
3. Hardware-validate with t04 and t05 (both need BTNC, pin P16). If either
   fails, compare the emitted FF control FASM against the X-Ray DB
   segbits_clbll_l.db feature by feature, and against a Vivado bitstream
   for the same FF (see ZB-10).
4. Keep comments short: one short block per function, one line per test at
   most. The user corrected long PR comments on Nov 4, 2026 for SHIELD
   firmware work; the same standard applies here.

## Files and commands

- Code: crates/helion-xilinx7/src/emit.rs (FF emit loop, ZRST /
  CEUSEDMUX / SRUSEDMUX), src/pack.rs (PackedFf, lines 28, 236-240).
- DB: ~/workspace/xilinx-dbsrc, segbits_clbll_l.db.
- Tests: ~/workspace/zedboard-hw-loop/tests/t04_ff_ce.sv,
  t05_ff_sr.sv, test_manifest.md (t04, t05 sections). Pin map:
  examples/zedboard.xdc (BTNC P16, LD0 T22).
- Uncommitted straggler, do not commit without the user's word:
  crates/helion-xilinx7/tests/ff_control_bits.rs and
  examples/zedboard_static.sv, plus a 3-line comment-only change in
  emit.rs (asked Oct 22, 2026, unanswered).

## Dependencies

Depends on ZB-01 (the clock must work first; CE/SR tests are clocked).
Related to ZB-02 (BUFG config) and ZB-10 (Vivado diff as ground truth).

## Rules

- No em dashes anywhere.
- Never fabricate. Hardware validation is the proof; unit tests are not.
- Comments stay short. Fix the code correctly first.
