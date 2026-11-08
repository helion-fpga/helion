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

## First run (Nov 8, 2026, Mac)

Pair: Helion examples/blinky_reg.sv build
(/tmp/x7/blinky_reg_helion.bit, 4,045,666 bytes, from
crates/helion-xilinx7/tests/blinky_reg_e2e.rs) vs Vivado
~/CDA4253_A1/blinky.bit (= work/ref/GT-blinky.bit, md5 3f6d7cfa...,
verified before the run). Same RTL, same part, same pins (clk Y9,
LD0 T22). Commands:

- `cargo test --release -p helion-xilinx7 --test blinky_reg_e2e`
  (no env setup; DB auto-resolved)
- `python3 work/rawdiff.py /tmp/x7/blinky_reg_helion.bit
  work/ref/GT-blinky.bit --json /tmp/x7/blinky_reg_diff.json`

Result: 4,741 differing bits in 3,219 clusters (4,391 NAMED,
350 UNNAMED, 15 in unclaimed frames). Full list in
/tmp/x7/blinky_reg_diff.json. Tool note: rawdiff walks the full
device frame map, so both inputs must be full-device payloads; the
pre-fix partial Helion payload fails with "payload exhausted".

Classification:

- (a) Owned by ZB-02: ZINV_CE1/ZINV_S1 set by Helion only;
  IS_IGNORE1_INVERTED and CLK_BUFG input-mux pips set by Vivado
  only; ZPRESELECT_I0 set by Helion only (open question). Details
  in ZB-02.
- (a) Owned by ZB-04: Vivado implements the counter as a CARRY4
  chain (PRECYINIT, ACY0, 0xffffffff00000000 LUTs); Helion builds
  it from LUTs. All chain-slice diffs trace to this.
- (b) New gap, needs a file: HCLK region clock buffers. Vivado
  sets word-50 bits across HCLK_R, HCLK_IOI3, CLK_HROW_BOT_R
  (hundreds of ref=1 ours=0, all UNNAMED); Helion emits zero HCLK
  bits.
- (c) Placement freedom: CLB/INT routing diffs outside the items
  above. The two placers put logic in different slices, so raw
  positions differ even where encodings agree. A canonicalised
  per-cell comparison is still open.
- Not done: zero diff. This run is the baseline all fixes measure
  against.

## Second run (Nov 8, 2026, Mac, after the ZB-02 emit fix)

Same pair, rebuilt Helion side. Result: 4,737 differing bits
(4,387 NAMED, 350 UNNAMED, 15 unclaimed), down 4 from 4,741.
Record in /tmp/x7/blinky_reg_diff2.json. The BUFGCTRL config block
is zero differing bits. Remaining, in order of attack:

1. Clock input/output routing (router): input-mux pips
   CK_MUXED0/IMUX24_0/IMUX28_0, output pip BUFGCTRL0_O to GCLK0,
   REBUF enables. Helion detours through GFAN/GCLK instead.
2. HCLK region buffers (new gap, needs a file).
3. ZB-04 carry chain vs LUT counter.
4. Placement-freedom routing (needs canonicalised comparison).
