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
   CK_MUXED0/IMUX24_0/IMUX28_0 DONE via general BFS (third run);
   still open: output pip BUFGCTRL0_O to GCLK0, REBUF enables.
2. HCLK region buffers (new gap, needs a file).
3. ZB-04 carry chain vs LUT counter.
4. Placement-freedom routing (needs canonicalised comparison).

## Third run (Nov 8, 2026, Mac, input routing fix)

route.rs routes IBUF to BUFG through general BFS pips instead of
the bitless ClockSpine case. Result: 4,732 (record
/tmp/x7/blinky_reg_diff3.json). The BFS found Vivado's exact input
corridor: input-mux bit plus 4 HROW/CMT input bits matched, zero
new ours-bits. All ref-side BUFG/REBUF bits still open at this
point: O to GCLK0 pip, REBUF enables and stitch.

## Fourth run (Nov 8, 2026, Mac, spine stitching)

New BufgSpine path (xdevice + route + emit): O exit to GCLK0,
TOP to BOT stitch conducted downward, REBUF ENABLE_ABOVE/BELOW
following entry-plus-edge-feed pattern. Pinned by unit test
bufg_spine_paths. Result: 4,726 (record
/tmp/x7/blinky_reg_diff4.json, JSON in
/tmp/x7/blinky_reg_diff5.json after the stitch flip). Every
ref-side BUFG/REBUF bit now matches. Remaining BUFG-tile diffs
are 10 Helion-only tie-off clusters: Vivado ties off with zero
bits in these frames (its tie-off selections unknown).

## Fifth run (Nov 8, 2026, Mac, IOB package)

Two fixes: site_rel maps IOB BEL by site type (IOB33M master goes
to IOB_Y0, IOB33S slave to IOB_Y1; pinned by
iob_site_rel_master_slave), and emit adds the Vivado IOB package
(DRIVE I12 via I12_I16, input FAST slew, PULLTYPE/IN_TERM NONE,
ILOGIC ZINV_D, OLOGIC OQUSED + OMUX.D1 + DATA_RATE_TQ.BUF;
pinned by emit_iob_vivado_defaults). Decoded exact values from
the oracle with work scripts (note: DATA_RATE is BUF, not SDR).
Result: 4,698 (record /tmp/x7/blinky_reg_diff6.json for the BEL
fix, /tmp/x7/blinky_reg_diff7.json for the package). The IOB
block (both pins, config + logic + pull + drive + slew) is zero
differing bits.

Regression note: the package first broke zedboard_e2e two ways,
both fixed same day. (1) Bare "IOB33" SING sites have no
master/slave marking: site_rel falls back to site position there
(UNPROVEN, ZB-09 owns SING truth). (2) SING IOI tiles have no
logic segbits: ioi_tile returns None for SING pairs, so no
OLOGIC/ILOGIC lines there (error was "no segbits for feature
RIOI3_SING.OLOGIC_Y0.OMUX.D1"). Record /tmp/x7/blinky_reg_diff8.json
confirms 4,688 on the final build.

## HCLK verdict (Nov 8, 2026, Mac): BLOCKED on new fuzzing

Vivado's HCLK word-50 bits verified by hand (tile HCLK_R_X175Y26,
offset 23, bits 0/4/5/6) against segbits_hclk_r.db: the file
covers frames 00 to 05 only, the tile has 26 frames. Row-buffer
bits are outside public data. All ~350 UNNAMED bits trace to this
class. Needs Vivado fuzzing (license). Not grindable further here.

## Sixth run (Nov 8, 2026, Mac, after ZB-04 stage 1 commit 9f90549)

Same pair, rebuilt Helion side (`cargo test --release -p
helion-xilinx7 --test blinky_reg_e2e`, env unset, output
4,045,666 bytes). Oracle md5 re-verified 3f6d7cfa before the run.
Result: 4,688 (record /tmp/x7/blinky_reg_diff9.json: 4,338 NAMED,
350 UNNAMED, 15 unclaimed), identical to run five. Expected: the
stage 1 carry path is opt-in (`synth_sv_carry`), the default flow
that builds this bitstream is untouched, so this run is a
no-regression confirmation, not a reduction. The diff moves only
once the backend consumes Carry4 (pack pin nets plus place, route,
emit), which is the next stage.

## Eighth run (Nov 8, 2026, Mac, anchored convergence)

Carry bitstream (/tmp/x7/blinky_reg_carry.bit, head anchored on the
golden head slice SLICE_X113Y39) vs oracle: 1,812 differing bits in
1,070 clusters (record /tmp/x7/carry_vs_oracle_diff1.json: 1,462
NAMED, 350 UNNAMED, 15 unclaimed), down from 4,688. Composition:
784 LUT_INIT, 432 plus ~100 OTHER routing (INT/GFAN/LOGIC_OUTS: Q
fanout, CYINIT AX route, LD0 cone), 90 FF_CFG (slice positions),
20 CLOCK, 5 CARRY_CFG (pad-slice CINs at Y43-Y45 where golden
extends past our chain end), 350 UNNAMED (HCLK, license-blocked).
Head lines match golden exactly (PRECYINIT.AX plus ACY0 on
CLBLM_R_X71Y39.SLICEL_X1). Independent check: work/carrysim.py
judges our carry bitstream a working counter with toggling top bit
(same instrument that validates the oracle; reader caveat: its
INIT reader underreports some high bits, verdict direction robust).

Decoded this round (raw frame words, not the filtered views):
unconnected LUT inputs read LOW (proven by elimination: pad S-LUT
I5 pins and head AX have no driving pips, no TIEOFF, no VCC, yet
silicon counts; float-high breaks the pads, float-low fits all
evidence). Pads (X1@Y40-43 S-LUT complements plus CINs) are
transparent pass-throughs (S=0 gives CO=CY). Head CYINIT=1 comes
from X0-ALUT (0x0f0f0f0f0f0f0f0f = NOT-A3 with A3 unconnected,
const 1) into AX; our flow instead fans one-LUT to AX through
fabric (proven working, bit-different). The O5-vs-DI question is
settled per prjxray site JSON: CO = S ? O5 : CY, and O5 is const 0
for passthrough INITs, so our DI-tied-low sim model is exact for
this design (general O5 eval is a later sim upgrade).

Next: S-input routing convergence (per-input IMUX mapping from INT
pip decode) plus transparent pad emission, then the head const-1
ALUT. HCLK UNNAMED still waits on the license.

## Ninth run (Nov 8, 2026, Mac, pads)

Same anchored carry pair with transparent pads (commit 1734c0c):
1,264 differing bits in 868 clusters (record
/tmp/x7/carry_vs_oracle_diff2.json: 914 NAMED, 350 UNNAMED, 15
unclaimed), down from 1,812. Composition: 568 ROUTING, 272
LUT_INIT, 58 FF_CFG, 12 CLOCK, 3 OTHER, 1 CARRY_CFG, 350 UNNAMED.
Pads closed 4 CARRY_CFG plus ~510 LUT_INIT/FF bits; placed order
matches the golden slices exactly (X1@Y39, X0/X1 @Y40-43,
X0@Y44, X1@Y44, asserted in the e2e). Last CARRY_CFG bit is the
LD0-tail tile (Y45 SLICEM PRECYINIT: golden LD0 slice carries CIN,
ours places LD0 generically). carrysim judges the pad-carrying
build a working counter (plus a fixed tool crash when the sibling
half outnumbers the target lanes); oracle re-validates unchanged
after the tool fix. Sim proves pads transparent (70 cycles
cone-identical with 4 pads in the chain).

## Seventh run (Nov 8, 2026, Mac, after full backend carry-through)

Same pair, rebuilt default-flow Helion side (all three bitstream e2e
green). Result: 4,688 (record /tmp/x7/blinky_reg_diff10.json),
identical again. Expected: the carry flow is still opt-in, so the
default bitstream is unchanged; this run proves the place/route/emit
carry changes are no-ops for designs without Carry4 cells (87 lib
tests plus all e2e green). A separate carry bitstream now exists at
/tmp/x7/blinky_reg_carry.bit (opt-in flow, golden-shaped chain
features, proven by blinky_reg_carry_e2e_fasm). Its raw diff against
the oracle is not yet meaningful: our placer puts the chain in a
different column (placement freedom, open item), so positions differ
even where encodings agree. Next: placement match to the golden
column (CLBLM_R_X71Y39 upward), then a canonicalised per-cell
comparison.
