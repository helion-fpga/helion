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

Clock-architecture note (Nov 8, 2026): the ref-only column bits are
dominated by clock DISTRIBUTION (GFAN0/GFAN1/CLK0/CLK1 fanout per
INT tile plus FFSYNC per FF), which our ClockSpine path never
emits: our slice CLK arrivals are unconfigured (zero FFSYNC in our
FASM), so no Helion bitstream can clock its FFs on silicon yet.
Golden also uses a BUFHCE in the HROW tile that our spine lacks,
and four GCLK branches (B0-B3) plus GCLK_B11 feed. Fixing arrival
is a correctness stage of its own (route slice-CLK endpoints plus
emit FFSYNC and the per-tile fanout), ahead of any hardware blink
claim. Reader note: work/carrysim.py docstring quotes a stale
X0-ALUT value; live reads agree with the raw-word decode, and the
INIT index padding (`INIT[00]`) explains the apparent reader gap.

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

## Tenth run (Nov 8, 2026, Mac, 892 to 557)

Six diff-grounded commits on fm/zedboard-fix-router (all gates green,
never master): BYP_ALT reservation for clock arrival (f5b9e12, 892 to
710: signals camped on clock-arrival muxes, forcing emit to drop the
uniform GFAN program); slice defaults ZRST/NOCLKINV/SLICEM-DI1MUX
(861b2ec, 710 to 684: only ZRST carries set bits, the other two are
clear-bit features already matched); BUFGCTRL parking (3ff0d2f, no bit
change, all-clear parks); fabric BUFG tieoff deletion (1f969bf, 684 to
674: device-wide power-IMUX scan of golden finds zero tieoff
programming; IGNORE1 masks the low pins); carry top-load LD0 IobOut
(53a5523, 674 to 665: the pin was dark, top-load skipped IOB emit);
transit B/C/D S-LUTs (2d556c9, 665 to 557: golden Y39X1 full group,
AND-idempotent covered-Q samples; golden IMUX13/IMUX34 selections
emerged from correct nets). Record /tmp/x7/carry_vs_oracle_diff21.json:
557 bits (89 ours-only, 468 ref-only), 207 NAMED, 350 UNNAMED.

Remainder classification (all verified by decode, no gaps except
noted). (1) Y39 head A pair (~100 bits): golden X0-ALUT is NOR(Q0
via A3, float A4) and X1-ALUT is AND(float A6, Q1 via A4); ours uses
A6 passthroughs. Replicating needs CYINIT/DI/A4/A6 float polarities,
which no decode can supply; our own silicon correctness already bets
on CYINIT-high. BLOCKED on hardware: propose a ZB-01 float-character
design (LUT sensing a float input routed to an LD pin, plus CYINIT
variants). (2) S-feed detours (west/east/south long lines, LD0 north
dogleg): mid X1 proven FF-less AND positions (no FFs Y40-43 X1 by
decode), so their S choices are don't-care by AND-idempotence;
X0 (FF'd) IMUX selections already match (6 of 8 per mid tile), only
upstream hop detours differ (timing class, needs a delay model).
(3) HCLK row (~350 UNNAMED): segbits covers 6 of 26 HCLK frames,
blocked on Vivado fuzzing license. (4) Structurally parked small
items: Y45 BYP_ALT0/2/5/7 unset and Y39 BYP_ALT1 unset (arrival
quirk, no rule found); corridor EL1 to Y39-BYP_ALT0 dangling stub
(far-east driver, not replicable by rule); CFG_CENTER ALWAYS_ON
props (3 set bits, single-sample evidence, part-specific tile
naming); OLOGIC/ILOGIC mode defaults (clear bits, zero diff effect).
S24 scare of this run was refuted by probe (own-Q24 end to end;
a Y39/Y44 misread); determinism re-verified (3 identical FASM hashes).

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

## Eighth run (Nov 8, 2026, Mac, S24 detour root cause)

Question: golden feeds Y44 S24 via OUTS3,ER1BEG_S0,ER1END1,IMUX42
while ours uses a 5-pip FAN chain for the same net. Hypothesis was a
router graph gap (missing stitch or pip). Disproven end to end:

- The OUTS3,ER1BEG_S0 edge exists in tile_type_INT_R.json.
- Independent Python union-find over tileconn.json plus tilegrid.json
  reproduces our stitched node exactly (8 members incl. the queried
  wire; ours reports the same 7 plus self). Stitching is bit-exact,
  not truncated. ER1BEG_S0 and ER1END1 are simply different nodes:
  the S0 turn joins row-45 line 0 east; END1 taps line 1 driven from
  INT_L_X70Y44.
- Full ER1 ring traced: OUTS3,WR1BEG_S0,WR1 node west,
  WR1END_S1_0,SR1BEG_S0,ER1BEG1 (all X70Y44), line-1 node east back,
  ER1END1,IMUX42 (Y44). 6 edges vs FAN 5. BFS by hop count
  legitimately prefers FAN.
- Contention map (same in both designs): SR1 carries the transit S
  feed (OUTS7) and EE4BEG3 carries an S-feed exit, so the 2-pip SR1
  path and the 3-pip EE4/WL1 dogleg are taken for both routers. The
  divergence is purely second-choice: FAN vs ER1 ring.
- GT2-blinky.bit (second Vivado build) is byte-identical to the
  oracle (0 diff), so there is no second placement sample.

Verdict: S-feed detours stay parked on a delay model. No code change
this run; bitstream and diff unchanged at 557. Keeper test added:
bfs_finds_shortest_s24_feed_on_real_graph pins the 2-pip SR1 path on
the real xc7z020 graph (lib suite 105 passed).

## Ninth run (Nov 8, 2026, Mac, CYINIT root cause, no hardware needed)

Question: PRECYINIT.C0 appears in both bitstreams, but our intent is
CYINIT=1 while golden inverts S0. Both cannot be right under one C0
meaning. Decisive evidence from Vivado's own synth netlist
(CDA4253_A1/vivado-on-silicon-mac/blinky_proj/blinky.runs/synth_1/blinky.dcp,
blinky.edf inside): head CARRY4 count_reg_0__i_1 has CYINIT on net
&_const0_. So C0 is const-0, and the helion-sv comment claiming
"proven 1" (lib.rs:16334) is backwards reasoning that must be fixed
with the construction.

Consequences, all verified by decode this run:

- Golden triple (correct incrementer): CYINIT=0, S0 = NOT Q0
  (X0-ALUT INIT = index mod 8 below 4; Q0 on A3 proven by
  CLBLM_IMUX1 to CLBLM_M_A3 in tile_type_CLBLM_R.json), DI0 = 1
  (no DI drive among all 261 plus 23 Y39 features, so undriven
  float-high, which the blink-proven oracle confirms reads high).
  Check: O0 = NOT Q0 xor 0 toggles, CO0 = (NOT Q0)?0:1 = Q0.
- Golden S1..S3 are plain Q passthroughs (X0 B/C/D INIT exactly bits
  32..63; routes IMUX12/IMUX35/IMUX43 already match ours), so X0
  differs only in ALUT (32 bits of the joint symmetric difference).
- X1-A is AND(Q1 via A4, float A6): INIT bits 40..47 plus 56..63,
  pin proven by CLBLM_IMUX10 to CLBLM_L_A4. X1 differs only in gate
  shape (16 bits), functionally identical under float-high.
- Our triple (CYINIT=0 via C0, S0 = Q0 A6-passthrough, DI0 =
  float-high 1): O0 = Q0 stuck, counter frozen. Our carry bitstream
  is dead on silicon by construction. helion-sim passes because it
  models the CYINIT=1 intent flag (sim lib.rs:162) and never reads
  the emitted C0 bit: control-bit semantics are a hole in the
  CARRY4 equivalence rule and must be covered by the fix test.
- Our DI0 is undriven too (21 DI1MUX lines, zero DI0MUX in our
  FASM), so float-high DI is already assumed by both designs.

Fix design for the next turn (construction switch, carry flow only):
flag head CYINIT 0, force head S0 LUT to NOT-Q on A3 and X1-A LUT
to the AND shape on A4 at pack level (same forcing class as the
transit trio), fix the emit mapping (flag 1 to C1, flag 0 to C0),
leave DI undriven, model undriven-high in helion-sim with a cone
equivalence test, then e2e plus rawdiff (expect about minus 60:
32 X0-ALUT plus 16 X1-ALUT plus A-route features). No rushed carry
edit this run per the wrong-Carry4 rule; bitstream and diff
unchanged at 557.

## Tenth run (Nov 8, 2026, Mac, head construction switch, 557 to 503)

Implemented the ninth-run fix design with one scoping change: X1-A
keeps a passthrough (I3-pass on A4, float-immune, sim-clean and
silicon-clean) instead of golden's AND shape, which needs a float-high
LUT-input rule in sim that collides with the standing equivalence
rule. The 16 AND-shape bits stay open honestly.

- helion-sv: head CYINIT flag 1 to 0; head S0 LUT to NOT-I2
  (0x0f0f0f0f0f0f0f0f) on I2/A3; transit S0 LUT to I3-pass
  (0xff00ff00ff00ff00) on I3/A4. Pins flow through the existing
  I-to-A index machinery, no backend pin code touched.
- pack: cyinit_c0 bool to cyinit Option<bool> with loud value
  parsing; route/place/emit fixtures to None.
- emit: value mapping (Some(true) to C1, Some(false) to C0) plus a
  synthetic mapping test; the old unconditional C0 was the
  silicon-freeze.
- sim: undriven DI0 on a CYINIT=0 config head reads high in both
  evaluators (AX-fed float path, golden-parity, oracle-blink
  proven). Both cone tests (blinky 5000-cycle, 8-bit wrap) pass on
  the new triple; they discriminate the rule (without it bit 1
  sticks at cycle 2).
- e2e: S-LUT passthrough count 49 to 47 plus explicit head and
  transit INIT asserts at the golden tiles. Router reproduced
  golden's exact IMUX1.LOGIC_OUTS4 and IMUX10.LOGIC_OUTS5; the old
  FAN/WR1 A-feed detours are gone.
- Gates: sv lib 228, sim lib 32, xilinx7 lib 106, all four e2e
  (blinky_bit, carry, default reg, zedboard) green; determinism
  re-verified (two identical FASM hashes across rebuilds).
- Record /tmp/x7/carry_vs_oracle_diff23.json: 503 bits (59
  ours-only, 444 ref-only). Y39-A closed except the parked 16
  AND-shape bits. Remainder: HCLK (~350, license), S detours
  (delay model, eighth run), X1-AND shape (16, sim float rule),
  arrival quirks plus corridor plus CFG_CENTER smalls.

## Eleventh run (Nov 8, 2026, Mac, pad-A sample divergence, 503 held)

Question: our pad-A route uses a 5-tile NW2 cascade (X72,
NW2BEG3 from WW4END3, 8 ours-only bits) while golden feeds pad-A
locally (Y40: IMUX5 from FAN_BOUNCE3 via FAN_ALT3 from NL1END_S3_0,
node-driven intra-tile by NL1BEG0 from OUTS5). Traced end to end:

- Different source nets, not a route divergence: ours samples the
  south top Q (Y40 pad-A from Y39 bit3 via EE4/WW4/NW2, the sv fill
  rule), golden samples the local group BQ (Y40 bit5 via NL1).
- Both transmit: brute-forced pad MUXCY cascade over all states
  shows ours {Q3,Q6,Q7,Q7} and golden {Q5,Q6,Q7,Q7} both pass CI
  through exactly when DI reads 0 (pads) with DI0=1 at the head.
  (An earlier by-hand model with the wrong pad CI failed both
  designs and disagreed with the passing 5000-cycle cone test; the
  error was using head CO0 instead of the pad CI, plus DI confusion.
  The brute force plus the green cone test is the record.)
- DI plumbing refined: DI gets no endpoints and no FASM lines in
  either bitstream. DI0 reads the undriven AX path high (the oracle
  needs DI0=1 for head CO0; an O5-read would give NOT Q0 and break
  it, refuting the old route.rs O5 comment, corrected this run).
  DI1-3 read low (LUTRAM MC defaults), which pads need. Sim models
  exactly this split and the cone tests prove it.
- Verdict: the sample choice (nearest-local vs south-covered) is a
  wirelength/timing cost decision, same bucket as the eighth-run
  detours. A future fill-selection rule could minimize BFS distance
  in our own graph without delays, but hardcoding local-BQ from one
  sample is not a rule. Parked with the cost-model work; no behavior
  change this run, diff held at 503 (re-verified same artifacts).

## Twelfth run (Nov 8, 2026, Mac, X1-AND shape, 503 to 487)

Closed the 16-bit X1-AND item under the resumed force-to-zero order:
transit S0 LUT to AND(Q1 on I3, float A6 on I5), golden Y39X1 shape
0xff00ff0000000000. Functionally exact under float-high (proven for
both pin readings), with the route (IMUX10 from OUTS5) unchanged.

- sim: undriven pins on S-feed LUTs model float-high in both
  evaluators (new `s_feed_luts` set: LUTs whose O drives a Carry4 S
  input). A first global attempt broke 3 generic LUT tests that
  contract tie-low; the scoped rule keeps that contract (all 32
  green) while the carry cone tests prove the AND shape counts.
  Stale tie-low comments corrected in sim and sv (pad fill, transit).
- e2e: passthrough count 48 to 47 plus transit AND INIT assert at
  the golden tile; all four e2e green; determinism re-verified
  (identical FASM hashes across rebuilds).
- Gates: sv lib 228, sim lib 32, xilinx7 lib unchanged by this turn
  (no xilinx7 src edits; 106 green two turns ago still stands).
- Record /tmp/x7/carry_vs_oracle_diff25.json: 487 bits (43
  ours-only). Remainder: HCLK (~330, license), S detours plus pad-A
  fill plus arrival (cost model), corridor plus CFG_CENTER smalls.

## Thirteenth run (Nov 8, 2026, Mac, always-on port, 487 to 484)

Cross-build frame comparison showed word 68 set in every Vivado
build and every old Helion build but zero in ours, while word 50 is
GT-only (design-driven, stays parked). The word-68 bits are
CFG_CENTER_MID.ALWAYS_ON_PROP1/2/3, and an orphan Oct 23 commit
(1044433, on no branch) shows our line never ported the
`required_features()` mechanism: read the part directory's
`required_features.fasm` at load, emit unconditionally, warn if
missing. Ported exactly that (data-driven from the open DB, not
oracle-pasted): Device carries the list, EmitDevice defaults empty,
XDevice serves it, emit_fasm prepends it.

- Gates: xilinx7 lib 107 (new mapping test), sv 228 and sim 32
  untouched and standing, all four e2e green, determinism
  re-verified (identical FASM hashes across rebuilds).
- Record /tmp/x7/carry_vs_oracle_diff27.json: 484 bits. The 3 named
  CFG entries are gone; the 13 unnamed word-50 bits stay (license).

## Thirteenth run (Nov 8, 2026, Mac, multicast sharing, 484 held)

New mechanism finding inside the detour bucket. Y39-S3 (D-LUT,
samples bit3) shares the SR1 stem with transit-C (trs2, also samples
bit3): ours is IMUX42 from SR1BEG_S0 (2 pips), golden takes the ER1
ring for the same net. Same-net stem sharing falls out of
ownership-keyed claims (one driver, two taps: legal multicast), so
our router shares regardless of order; golden routes per-sink
exclusive (transit-first order artifact). Forcing exclusive claims
to match would break legal multicast, risk routability, and buy
nothing functional. Parked with the cost-model work; rebuild green,
rawdiff re-verified at 484, no behavior change.

## Tool-flow negatives (Nov 8, 2026, Mac, no-change record)

Two hypothesis classes closed from Vivado's own artifacts: the
journal shows bare `place_design` / `route_design` (default
directives, so detours are default behavior, not exploration), and
the placed utilization report shows FDRE 26 plus CARRY4 13 (exactly
our counts, so phys_opt replicated nothing). Structural parity at
cell level; divergence is purely placement-detail (matched),
INIT/route choice, and clock programming.

## Arrival census (Nov 8, 2026, Mac, no-change record)

Full 7-tile golden BYP_ALT program (gt_decode, one run): Y39 misses
{1} (ALT0 is the corridor stub), Y40-44 full house, Y45 misses
{0,2,5,7}. Per-slice clock-need mappings all die on Y39 (its X1
transit needs no clock yet keeps {0,2,5,7}), and vertical
branch patterns show no connected tree. The program reflects
Vivado clock-tree tap topology, unknowable without its clock
router. Parked; Y45 over-programming (10 ours-only bits) stays.

## Fourteenth run (Nov 8, 2026, Mac, CMT detour classified, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff28.json: 43 ours-only, 134
named, 350 unnamed). Remainder audit of the one unclassified
cluster: CMT_TOP_L_UPPER_T (14 records, 12 ours-only) is the left-side
mirror of the parked detour bucket. Ours drives the INT_L X72Y39-45
spans through NW2BEG3 / NE2BEG1 long lines; golden takes ER1BEG1
(the ER1 ring) for the same spans. Same route-choice class as the
right-side FAN-vs-ER1 detours: no grounded rule picks the ring
without a wirelength / delay cost model, so it parks with the rest.
Remainder stands: HCLK unnamed (~330, license), detours both sides
plus pad-A fill plus arrival (cost model), CFG_CENTER word-50 unnamed
(license), corridor plus complements. No behavior change.

## Fifteenth run (Nov 8, 2026, Mac, RIO slice classified, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
with identical cluster counts (record
/tmp/x7/carry_vs_oracle_diff29.json: 382 clusters, 134 named, 350
unnamed), confirming a deterministic rebuild. Remainder audit of the
RIO slice: the 12 RIOB33 / RIOI3 records are tile-overlap artifacts,
not IOB config divergence. Every one names INT_R route pips only
(NR1BEG1 endpoints, IMUX34 BOUNCE selections at X73Y45-46); no
OLOGIC / OMUX / IOB feature appears. Same parked detour class.
Gates: xilinx7 lib 107, sv lib 228, sim lib 32, all green with
pipefail-guarded result lines. No behavior change.

## Sixteenth run (Nov 8, 2026, Mac, contention probe negative, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff30.json). New probe of the
detour class: full segbits decode (gt_decode) of both bitstreams at
INT_R_X71Y44, plus a 22-tile census of multi-driven LOGIC_OUTS pins
in both. Golden: LOGIC_OUTS3 injects once into the ER1 ring
(ER1BEG_S0) and FAN_ALT3 taps back from it (NL1END_S3_0). Ours: the
same source fans out directly (FAN_ALT3.LOGIC_OUTS3) with the second
sink on a separate chain (IMUX5.NW2END3, IMUX42.FAN_BOUNCE7). Two
candidate rules died on the data: golden multi-drives LOGIC_OUTS in
25 cases across the 22 tiles (no single-load rule), and no wire is
driven twice anywhere (no contention forced the ring). The
divergence is a genuine inject-at-source vs inject-ring-then-tap
choice with no derivable rule from one sample, so it stays parked
with the cost-model work.
Gates: xilinx7 lib 107, sv lib 228, sim lib 32, all green with
pipefail-guarded result lines. No behavior change.
