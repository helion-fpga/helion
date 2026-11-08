# ZB-10: Vivado-vs-Helion bitstream diff on the blinky

- Track: ZedBoard, validation. Ground truth for ZB-02, ZB-03, ZB-09,
  and every future backend fix.
- Branch: none (never run). Worktree: ~/workspace/helion-zed-bringup.
- Status: never run.

## Goal

Diff a Helion bitstream against a Vivado bitstream for the same design,
bit by bit, and explain every difference. This is the ground truth that
settles arguments about BUFG polarity, FF control bits, and missing
segbits.

## Done when

A Helion bitstream and a Vivado bitstream for the same blinky design are
diffed frame by frame, and either the diff is zero, or every differing
bit is listed with an explanation (which fix owns it, or that it is an
unknown that needs a fix). A diff that is run but not explained is not
done.

## What is already done

- The Vivado side is proven: a Vivado blinky.bit programs the board and
  blinks (SHA256 e04036ec...). The Helion side is proven to program
  (done=1) and go dark. The two bitstreams exist; they have never been
  compared.
- The UltraScale+ track has the same job at a larger scale (US-08: tier 2
  Vivado diff for 100 designs) and has already built the diff harness
  pattern (canonicalisation, per-block diff reports, placement hash and
  seed recorded per run). Reuse that pattern; do not build a second,
  different differ.
- The 7-series frame layout is known: the backend has frames.rs (frame
  assembly) and bitstream.rs (the .bit writer), both reviewed clean by the
  Oct 22 investigator.

## What was tried and what failed

- Nothing tried. No diff has ever been run on this part.

## What to do

1. Locate the Vivado blinky.bit used on Oct 22, 2026 and the exact
   Helion bitstream to compare it against (start with the v3 tie-off
   bitstream, ~/workspace/zedboard-hw-loop/bitstreams/zedboard_blinky_v3_tieoff.bit).
   Confirm both are for the same RTL (examples/zedboard_blinky.sv) and
   the same pin map (examples/zedboard.xdc). If the Vivado bitstream
   cannot be found, say so and stop; do not compare against a bitstream
   from a different design.
2. Build the differ by adapting the UltraScale+ harness pattern:
   parse both bitstreams to frames, canonicalise, and report per-block
   diffs (BUFG, CLB, IOB, BRAM, clocking) with frame / word / bit
   positions.
3. Produce the diff report. For each differing bit, classify it: (a)
   owned by a known fix (name the sub-problem), (b) a new bug (file it
   as a new sub-problem with the exact position), or (c) unknown.
4. Feed the results back: every (a) goes to its sub-problem as ground
   truth, every (b) becomes a new sub-problem file in this directory.
5. Record the exact commands and the diff report in the handoff folder so
   the diff is reproducible.

## Files and commands

- Helion bitstream: ~/workspace/zedboard-hw-loop/bitstreams/zedboard_blinky_v3_tieoff.bit
- RTL: examples/zedboard_blinky.sv. Pin map: examples/zedboard.xdc.
- Backend parsing: crates/helion-xilinx7/src/frames.rs,
  src/bitstream.rs, src/fasm.rs.
- Diff pattern: ~/workspace/ultra-bitgen-crew/harness/ (diff.py,
  diff_engine.py) and crates/helion-ultrascale/harness/.

## Dependencies

Depends on the Vivado bitstream being located. Unblocks ZB-02 (BUFG
polarity ground truth), ZB-03 (FF control bits), and ZB-09 (SING tile
frames). Related to US-08.

## Rules

- No em dashes anywhere.
- Never fabricate. A bit is either identical or it is not; report the
  positions.
- Do not compare bitstreams from different designs or different pin maps.
