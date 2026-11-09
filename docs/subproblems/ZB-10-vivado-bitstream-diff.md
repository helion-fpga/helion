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

## Seventeenth run (Nov 9, 2026, Mac, multi-sample word-50 campaign, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff32.json, bit-identical to diff31:
484 bits, 382 clusters, 134 named, 350 unnamed). Deterministic rebuild
confirmed across days.

GT2 killed as a second sample: rawdiff GT-blinky.bit vs GT2-blinky.bit
is 0 bits (record /tmp/x7/gt_vs_gt2.json). The md5s differ only in .bit
header timestamps; Vivado 2023.2 is payload-deterministic for this
source, so GT2 carries no new information.

New independent samples from the A3 coursework builds (all Vivado
2023.2, 7z020clg484, md5-verified): fa4b_dataflow (a2bfcb03),
fa4b_structural (fec193838, same design, different RTL style), and
bcd_2_bin (65430953). Probes in /tmp/x7/w50_probe.py, w50_fullmap.py,
w50_correlate.py, w50_tilecorr.py, fadiff.py (scratch, uncommitted).

Findings, all measured:

- Word 50 is the HCLK tile's own config word: every column frame gives
  50 tiles words 0-49, the HCLK tile word 50 (off=50, words=1), and 50
  tiles words 51-100. Confirmed in tilegrid claims and in the
  prjxray configuration docs (101 words: 100 for connected tiles, 1
  for the horizontal clock row).
- The 350 word-50 diffs split four ways across the samples: 46 set in
  ALL four Vivado bitstreams (all 13 CFG_CENTER_MID bits plus 33 in
  HCLK_R_X175Y26 / HCLK_IOI3_X185Y26, record
  /tmp/x7/w50_universal.json), 255 only in blinky (clocked bottom
  region), 35 fa-only, 14 bcd-only. Chip-wide word-50 counts are
  GT 356, faD 554, faS 520, bcd 614, ours 6 (all 6 match GT).
- Each design programs the HCLK rows of the regions it uses: GT only
  the Y26 row, fa/bcd the Y78/Y130 rows plus right-edge IO tiles.
  faD vs faS (same design) agree everywhere except HCLK_R_X175Y78
  (122 vs 104 bits), while their used-tile sets are identical and
  differ by one route word in INT_R/CLBLM_R_X71Y94, the same column
  whose frames carry that HCLK tile's word. So the HCLK word tracks
  per-column route detail, not mere tile usage; per-row-usage and
  per-frame-ECC models both die on this pair (same rows used, six
  frames flip for a one-word route delta).
- Our FASM emits zero HCLK features and golden decodes zero named
  HCLK features on these tiles, so the 200-line segbits_hclk DB is
  silent on all of it. No derivable rule exists from four samples:
  the 46 universals stay unimplemented (a measured constant without
  a mechanism is hardcoding, not a rule), and the column-detail
  encoding needs controlled Vivado fuzzing or many more clocked
  samples to derive.

Remainder stands, reclassified with evidence: 46 universal-default,
255 clock-region route detail, 49 IO/bank route detail, 134 named
route choice. Gates: xilinx7 lib 107, sv lib 228, sim lib 32, all
green with pipefail-guarded result lines. No behavior change.

## Eighteenth run (Nov 9, 2026, Mac, B242 deconstruction plus fix hunt, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff33.json, identical to diff32).

BLINK-242 deconstructed, and it is a trap: rawdiff against GT is only
64 bits (record /tmp/x7/b242_vs_gt.json), all word-50 ref-only, zero
named, zero ours-only. Its config stream geometry is byte-identical
to GT (1011391 words, FDRI 1010808, same FAR sequence), so it is GT
frame content passed through Helion tooling minus 64 bits, not an
independent build. Its 292 word-50 bits are a strict subset of GT
(0 extras), including the same trailing 15. Do not use it as a
sample: any rule learned from it is oracle inheritance. The 64 it
drops (HROW 28, HCLK_IOI3 21, CFG 13, HCLK_CMT 2) mark the old
frame round-trip gap, not a rule.

Fix hunt across the remainder, all negative with cause:

- Ours-only named 43 (IMUX per-row bits Y39-Y44, FAN_ALT Y44,
  BYP_ALT Y45, CMT mirror 12) are our sides of already-parked
  route choices (ring-vs-source, arrival taps, detour mirror).
  No new rule.
- NO-TILE 15 are word 50 of trailing CLB-frame minors (col 71,
  minors 30-31 of 26-35, no w50 owner in tilegrid). Survey: each
  design programs trailing word-50 in exactly one column, its
  own (GT 15, faD 34, bcd 55). faD vs faS use the same trailing
  minors but different bits, so trailing words track per-column
  route detail like the HCLK words, not usage. No rule.
- Upstream prjxray-db segbits for all HCLK/HROW/CFG types are
  byte-identical to ours (fetched Nov 9). No new knowledge.
- Mask DB bits have zero overlap with any word-50 pattern.
  Not the mechanism.
- Our FASM clock/HCLK features match golden exactly (HROW
  BUFHCE 6, HCLK_CMT 1+3, HCLK_L ENABLE/LEAF 2, all decoded in
  GT). Nothing missing at FASM level.

Classification correction: the 13 CFG word-50 bits are universal
(set in all four Vivado samples, record /tmp/x7/w50_universal.json),
not GT-only/design-driven as the sixteenth-run note held. That note
used Helion builds (B242, OURS-r25) as negative evidence, which is
circular now that B242 is proven GT-derived. They stay unimplemented
anyway: N=4 universality without a mechanism is still hardcoding,
and the other 33 universals share a bank-33 common cause, so even
universality needs a no-bank-33 sample to confirm.

Remainder stands: 350 word-50 route detail (per-column encoding,
needs Vivado fuzzing), 134 named route choice (needs cost model).
Gates: xilinx7 lib 107, sv lib 228, sim lib 32, all green with
pipefail-guarded result lines. No behavior change.

## Nineteenth run (Nov 9, 2026, Mac, PIP-level route survey, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff34.json, identical to diff33).
Session goal reactivated by user request: Vivado-equivalent
bitstreams from the native bitgen for a variety of programs, plus a
ZedBoard blinky flash. Loop continues with yesterday's methods
(rebuild, rawdiff, measured probes, gates, log, commit).

PIP-level survey of the detour tiles (X69-73, Y36-48, both designs;
records /tmp/x7/pip_detour_gt_ours.json, /tmp/x7/chain_pips.json,
/tmp/x7/nets_gt.json, /tmp/x7/nets_ours.json; scratch scripts
removed after use):

- The 134 named bits collapse to 61 PIP features (34 GT-only, 27
  ours-only), all INT_R/INT_L. Placement, slices, INITs match 100
  percent (zero named CLB bits).
- M-datapath proven identical: every M-side IMUX (4/12/13/34/35/43)
  selects the same source in both designs in every row Y39-45, and
  all same-tile driver PIPs are shared (absent from the diff). The
  counter core Graham Bell identical in routing, not just structure.
- Only L-side IMUX42/IMUX5 (L-D6/L-A6) differ, in a rigid per-row
  pattern Y40-43: GT takes south-neighbor flop bits (IMUX42 from
  OUTS4 below via SW2 west plus ER1 ring east, IMUX5 from OUTS5
  below via NL1 north plus FAN tap), ours takes local/east bits
  (IMUX42 from OUTS7 same tile via SR1, IMUX5 from the X72 east
  detour via NW2). Each link verified against tileconn; NL1BEG_N3
  is local-only (same-tile FAN taps), NL1BEG0 north is occupied by
  OUTS5, which is why the south bit cannot go north directly.
- L-OUTS pins are undriven in Y40-43 in both designs, so L-lane
  outputs are unobservable there; only Y44-top feeds the LED path
  in GT. M-IMUX sources never include L-OUTS, so L values cannot
  reach the M chain through any configured input.
- LED path: GT drives LD0 from L-DFF(Y44) through the ER1 ring
  and NR1; ours drives it from M-AFF(Y45) through ER1 and NE2.
  Derived equations give both the same 2^24-equivalent blink
  (ours toggles its LD0 flop on the bit23 carry-out overflow).
- carrysim assumes own-Q for LUT A6 and never checks actual wires,
  so its verdict cannot adjudicate GT-vs-ours L-inputs; both pass
  it trivially. The count_24/LD0 flop assignment question (same
  BELs, possibly swapped logical bits) stays open: GT's Y45 SW2
  dead-ends in empty X70Y46 in-window, and odd-index BOUNCE plus
  some spans are absent from tileconn (the known tileconn gap).
  Closing it needs a wire-accurate sim or silicon, not more
  static tracing.
- Net-tracing lessons recorded: tileconn covers boundary wires
  only; regular spans verified empirically per link; a leaked loop
  variable once nested a probe record (repaired, records intact).

Remainder stands with upgraded mechanism (not cost-model-unknown):
350 word-50 per-column route detail (needs Vivado fuzzing), 134
named of which the M-core is proven identical and the rest is
L-shadow plus LED-path variants with no derivable rule. Gates:
xilinx7 lib 107, sv lib 228, sim lib 32, all green with
pipefail-guarded result lines. No behavior change.

## Twentieth run (Nov 9, 2026, Mac, delay PoC plus BYP survey, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff35.json, identical cluster
counts). Two new probes, both negative with cause.

Delay PoC (unparks then re-parks the cost-model question): the open
DB ships per-pip delays in tile_type_INT_R/L.json (3737 pips
covered), so edge weights looked derivable. Tested on the true
same-net S24 detour: ours (5-pip FAN chain, from our FASM)
sums to 0.165 typ against golden's ring at 0.191 before counting
the ring's two unmodeled wire hops (OUTS3 to ER1BEG_S0 0.044,
WR1END_S1_0 to SR1BEG_S0 0.056, SR1BEG_S0 to ER1BEG1 0.044,
ER1END1 to IMUX42 0.047). Delay weighting would not pick the
ring, so Vivado's choice here is not delay-driven and DB delays
cannot ground the detour rule. Killed with numbers, not assumed.

BYP tie-subset hunt (the ~10 ours-only Y45 arrival bits): golden
ties ALT {1,3,4,6} at Y45 and skips ALT1 at Y39 while we tie all
8; unset muxes sit at the ppips VCC_WIRE default (logic 1), so
this is a real 0-vs-1 programming divergence. Dead ends, each
measured: downstream consumption does not explain it (golden
Y45 ALT3/4/6 outputs unconsumed in-tile and in all three
neighbor tiles, yet tied); CLK presence does not (CLK1-only on
every arrival tile Y39-45 in both designs; golden Y44 also has
CLK0 exactly like ours); HCLK adjacency does not (INT column
continuous through Y45, HCLK rows at 26/78/130). Multi-sample
test failed for lack of samples: fa4b_dataflow and fa4b_structural
(md5-verified a2bfcb03, fec19383) program zero BYP/CLK bits
chip-wide (unclocked), and bcd_2_bin (65430953) has a single
GFAN-consumer tile and no arrival programming. No second clocked
sample exists, so the tie subset has no derivable rule from one
sample and stays parked. Scratch probe /tmp/x7/byp_survey.py kept
out of the repo.

Remainder stands: 350 word-50 route detail (fuzzing), 134 named
route choice (cost model or second sample), 15 unclaimed trailing
minors. Gates: xilinx7 lib 107, sv lib 228, sim lib 32, all green
with pipefail-guarded result lines. No behavior change.

## Twenty-first run (Nov 9, 2026, Mac, internet second sample, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff36.json). Per user direction,
hunted the internet for real Vivado 7-series bitstreams instead of
waiting on the license.

What worked: Red Pitaya ecosystem image (pavel-demin, 120MB,
20+ Vivado 2025.1 bitstreams, header-stamped part strings).
`led_blinker_122_88.bit` is our exact die (7z020, clg400 package:
same silicon, INT tiles identical). Dead ends logged honestly:
pavel's git repos are source-only (no .bit), PYNQ does not commit
base.bit, open-dpu-zedboard ships only .xsa, the extra course
blinky (blinky_proj, b06c9299) is payload-identical to the oracle
(0 rawdiff bits, header timestamp only), and the A3 adder samples
are unclocked. New reusable scratch tooling (kept out of the
repo): /tmp/x7/cmpdec.py (COMPRESS=TRUE FAR/MFWR/FDRI decoder,
MFWR-zero assumption asserted: 9922 writes, 0 nonzero words),
/tmp/x7/byp_survey.py, /tmp/x7/pipcensus.py.

Two-sample BYP verdict (GT 2023.2 clg484 vs RP 2025.1 clg400,
full INT censuses /tmp/x7/census_gt.json 23 tiles,
/tmp/x7/census_rp.json 126 tiles): the tie subset correlates
with nothing. RP's column uses {2,5,7}, {0,2,5,7}, {0,5} down
its rows against our {2-7}, all-8, {1,3,4,6}; chip-wide, zero
BYP_BOUNCE/BYP wires are consumed by any pip in either design,
so ties are pure parking with no consumer basis; CLK letter,
position, HCLK adjacency, and FF occupancy all dead (RP tiles
share identical X1 occupancy with differing subsets; RP also
cross-validates our X1-to-CLK0 mapping on an independent
sample). Five dead mechanisms across two samples: the subset
is Vivado router-state residue, derivable only by fuzzing.

Detour mechanism upgrade from GT fanout census: every source
drives exactly 2 loads (zero fanout variation), so the
ring-vs-source split is source selection for unobservable
L-lane sinks (golden ships distant bits, we sample locally;
same values, rate-equivalent LED), and S24 is a value-neutral
path variant. Both arbitrary without Vivado's cost function.

Remainder stands, all parked on outside inputs: 350 word-50
route detail (fuzzing license), 134 named route choice
(Vivado cost function), 15 unclaimed trailing minors. Gates:
xilinx7 lib 107, sv lib 228, sim lib 32, all green with
pipefail-guarded result lines. No behavior change (no test
change: log-only turn).

## Twenty-second run (Nov 9, 2026, Mac, word-50 pooled mining, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff37.json). Attacked the 350-bit
word-50 bucket with the 11-design internet haul (GT 2023.2 plus 10
Vivado 2025.1 same-die builds, md5/header-verified).

Tooling fixed and built (scratch, out of repo): plain-path FDRI
parser silently misreads COMPRESS=TRUE streams (found 125 garbage
frames in vna), so all loaders now branch on the header stamp;
new /tmp/x7/w50pool.py pools (route-words, w50) minor samples:
/tmp/x7/w50pool.json holds 4075 samples. faD/faS/bcd files
recovered from the course package (A3, md5 match): faD vs faS is
80 bits (20 named IMUX/INIT at X71Y94, 60 unnamed HCLK), but the
route delta is multi-word, not the hoped-for minimal pair.

Mining results, all negative with cause: bitwise-OR and
column-parity of same-frame route words are disjoint from w50
sets (0 and 17 exact of 3768); no single route word-index
predicts any w50 bit (3725 samples); apparent perfect
correlates (w50[16] == w73/77/93/97.31) collapse to 6 samples
at one frame where 5 bits move together. Gap scoped exactly:
covered HCLK features match ours bit-for-bit (HCLK_R 0=0,
HCLK_L 2=2 on GT tiles), and the open DB's HCLK segbits use
only frame offsets 00-05 while Vivado programs word 50 in
minors 0-25, so the whole bucket is unfuzzed HCLK minors 6-25
(plus beyond-05 minors generally). Second-sample structure
confirmed (RP programs word-50 only in route-nonzero frames on
exactly its used rows); the code itself needs fuzzing or a far
larger learnable dataset. HCLK_CMT tiles program zero word-50
despite routes (control data point).

Remainder stands: 350 word-50 (unfuzzed HCLK minors 6-25),
134 named (cost function), 15 trailing minors. Gates: xilinx7
lib 107, sv lib 228, sim lib 32, all green with pipefail-guarded
result lines. No behavior change (log-only turn).

## Twenty-third run (Nov 9, 2026, Mac, LED-tap structure, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff38.json). Chased the LED-path
bucket to the bottom and it is structural, not a rule.

Our LD0 flop is Y45-X0-AFF (D = XOR of S and carry24); golden's
is Y44-L-DFF (D = Q24 via ER1 ring tour back into its own tile).
IOB proximity predicts ours (2 tiles away) over golden (3), so
proximity is dead. Mid-investigation a dark-LED pulse was derived
from a misread S wire (S assumed GND-tied) and then disproved
from our own FASM lines 507-513: OUTS4 to NL1BEG_N3 to FAN_ALT1
to FAN_BOUNCE1 to IMUX4 to M_A6, with ALUT INIT upper half all
ones, gives S0 = LD0's own Q. So ours toggles on overflow
(period and duty equal to golden's Q24 square, one cycle phase
lead): no silicon bug, and the standing sim/test story holds.
The existing test gap is noted honestly: 5000-cycle cone
comparisons never reach 2^24, and the toggle test pins the
S-feedback config, so the pulse-vs-toggle distinction would
have been caught by neither had S really been 0.

Bit-identity consequence: matching golden needs a route-fed D
flop at Y44-L plus the perverse same-tile ring tour, which is
unlearnable from any sample count. LED bucket parked with
cause. Side consistency: our Y45 GFAN ties feed no pip at all,
matching the tie-subset verdict that ties are consumerless
parking.

Remainder stands: 350 word-50 (unfuzzed HCLK minors),
134 named (cost function plus structural LED), 15 trailing
minors. Gates: xilinx7 lib 107, sv lib 228, sim lib 32, all
green with pipefail-guarded result lines. No behavior change
(log-only turn).

## Twenty-fourth run (Nov 9, 2026, Mac, corridor traced, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff39.json). Traced the Y39
corridor to its mechanism and killed two theories doing it.

Direction semantics first (BEG drives, END sinks): the corridor
net originates in the design's east (an OUTS4 S/shadow bit via
SW2BEG0 at X71), runs west to X69 (ER1BEG1 from WW2END0), and
comes back east into Y39-BYP_ALT0, dead at both ends (X68/X67
and Y38/Y40 neighbors decode to zero features; BYP outputs
consumerless chip-wide). The drive-by-clock theory died on the
trace: it is an S-net stub tour to X69 and back, not clock, and
the RP sample has no analog. Single-sample vestige, no
derivable rule. Side note: our clock input jumps IBUF to BUFG
via the dedicated CK_MUXED network without touching fabric,
so even the corridor's neighborhood differs structurally.

Our-only emission arithmetic also checked and rejected: our Y45
BYP ties feed no pip at all, so principled non-parking would
emit zero ties, but golden keeps four, netting zero diff change
while moving bits. Correctly left alone.

Remainder stands: 350 word-50 (unfuzzed HCLK minors 6-25),
134 named (cost function, structural LED, S-stub corridor),
15 trailing minors. Gates: xilinx7 lib 107, sv lib 228, sim
lib 32, all green with pipefail-guarded result lines. No
behavior change (log-only turn).

## Twenty-fifth run (Nov 9, 2026, Mac, LD0 correction, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff40.json). This run corrects
a standing survey claim with decoded evidence.

Flop census forces it: all 26 flop muxes are XOR-fed in BOTH
designs (golden Y44 X1-D plus X0 A-D and Y45 X0-A verified by
decode; ours identical BEL for BEL), so golden's LD0 flop is
Y45-X0-AFF (toggle on overflow via S-feedback), exactly like
ours. The nineteenth-run note's "golden drives LD0 from
L-DFF(Y44)" is wrong; Y44-L holds counter bit 23 in both.
Proximity as a rule died earlier anyway (it predicts ours).

The real LED divergence is same-net routing, proven endpoint
to endpoint: both drive Y45-AFF.Q to X73Y46-IMUX34 to OLOGIC
D1 to the IOB. Ours goes direct (same-tile ER1 east, NE2 hop
via X72Y45). Golden tours (OUTS4 southwest via SW2BEG0 to
X70Y44, ER1 east along Y44, climb back to Y45, ER1 through
X72Y45, NR1 north). Same source, same sink, scenic vs direct:
pure cost-function arbitrariness, same class as S24, parked
forever. No structural difference remains in the LED bucket.

Remainder stands: 350 word-50 (unfuzzed HCLK minors 6-25),
134 named (cost function, scenic-tour detours, S-stub
corridor), 15 trailing minors. Gates: xilinx7 lib 107, sv lib
228, sim lib 32, all green with pipefail-guarded result lines.
No behavior change (log-only turn).

## Twenty-sixth run (Nov 9, 2026, Mac, trailing minors, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff41.json). Attacked the 15
trailing-minor bits plus the universal-constant question with
the 12-design pool, and found the frame-ownership answer.

Two false alarms died by arithmetic before acting: our supposed
stray high bits in trailing words are set in golden too (bit 18
shared, XOR clean), and the JSON was right all along. Real
finding: trailing word-50 bits exist with zero decoded HROW
features (RP blinker programs them with none), so the trail
bits ARE HROW programming living entirely in unfuzzed trailing
minors 26-28, same gap class as HCLK. The trail pattern tracks
clock topology (BUFHCE sites, GCLK selects) across 16 HROW
tiles in 11 designs, but learning that mapping needs
clock-topology modeling, so it parks with the fuzzing bucket.
Universal constants (CFG 0x40191a/b identical across GT, RP,
faD, bcd) noted with strengthened N but left unimplemented per
the standing anti-hardcode rule. Tooling trap fixed along the
way: the plain FDRI parser silently returns garbage frames on
compressed streams, so all loaders now branch on the header
stamp (/tmp/x7/w50pool.py, hrowcensus.py).

Remainder stands: 350 word-50 (unfuzzed HCLK minors),
134 named (cost function, scenic tours, S-stub), 15 trailing
(unfuzzed HROW minors, clock-topology modeled). Gates: xilinx7
lib 107, sv lib 228, sim lib 32, all green with pipefail-guarded
result lines. No behavior change (log-only turn).

## Twenty-seventh run (Nov 9, 2026, Mac, universals verdict, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff43.json). Put the universal-constant
question to all 14 same-die designs (GT, faD, faS, bcd, 10 RP 7z020
builds, two Vivado versions, two packages).

HCLK universals: dead. Zero word bits survive 14 samples (the
4-sample claim was route-detail coincidence), killing a
would-be hardcode with data. CFG universals: 16 bits hold at
14/14 (13 word-50 plus 3 word-68, frames 0x40191a/b), ours has
zero of them. Decision, held deliberately: left unimplemented.
Sample-only constants without a bit-to-meaning mapping are
hardcoding no matter how high N gets, and that rule came from
the user directly. The required-features precedent does not
transfer (those are DB-named features; these are not). Logged
as falsifiable: a meaning assignment or a fuzzing license flips
it. Gates: xilinx7 lib 107, sv lib 228, sim lib 32, all green
with pipefail-guarded result lines. No behavior change (log-only
turn).

## Twenty-eighth run (Nov 9, 2026, Mac, HROW gap scoped, 484 held)

Rebuilt with no env setup (e2e green), rawdiff re-verified at 484
(record /tmp/x7/carry_vs_oracle_diff44.json). Followed the
trailing-word lead into the HROW segbits and scoped the gap
instead of closing it: the open DB covers BUFHCE plus CASCO mux
selects fully in trailing minors 26-29, our six HROW features
match golden's exactly, yet seven trailing bits differ. Those
bits belong to non-BUFHCE/non-CASCO HROW functions the DB never
fuzzed (likely GCLK segment enables), so learning them needs
row-to-bit mapping work at fuzzing grade. Open-fasm2frames
route skipped honestly: the tool needs venv deps we do not
have, and it caps at the same DB ceiling anyway, so it could
only audit our encoder, not reveal new bits. Gates: xilinx7
lib 107, sv lib 228, sim lib 32, all green with pipefail-guarded
result lines. No behavior change (log-only turn).

## Twenty-ninth run (Nov 9, 2026, Mac, Vivado fuzzing window opened, 484 held)

The SSH tunnel to the USF cluster is live and real Vivado runs
there, so the license UNKNOWN blocker is being settled by
experiment instead of speculation. Resolved the install layout
(nested Vivado/2017.3 tree) and confirmed Vivado 2017.3 starts
(Vivado v2017.3 SW Build 2018833). The cluster env already sets
LM_LICENSE_FILE=27000@license2.rc.usf.edu, which Vivado reads.
Also found a 2022.1-ep11 install, so both versions build the
same RTL for a version-sensitivity comparison. No behavior
change in Helion (log-only turn); diff baseline stays 484.

Campaign (all jobs build xc7z020clg484-1 from the oracle RTL
plus variants, non-project flow, full outputs: synth/placed/
routed .dcp, route/timing/util/clock reports, routed Verilog,
routed XDC with ROUTE strings, .bit, .bin):
control17 (job 25297) blinky on 2017.3, control22 (25298)
blinky on 2022.1, dr-place17/dr-route17/dr-xnd17 directive
sweeps, ck3-17 (MMCM 3-clock design, multi-BUFG HCLK variety),
bufh-17 (BUFHCE regional clock, HROW coverage), bufgloc-17
(BUFG forced to BUFGCTRL_X0Y1). All 8 running across
sccompute0-2. Next: pull routed XDC/DCP/bit files, derive the
HCLK minor 6-25 and HROW non-BUFHCE mappings plus route-choice
stats from Vivado's own outputs, then implement the next
diff-grounded fix. Gates: suite re-verification deferred to
the fix turn (no code touched).

## Thirtieth run (Nov 9, 2026, Mac, HROW word-50 code being derived, 484 held)

Rebuilt with no env setup (blinky_reg e2e green), rawdiff
re-verified at 484 (134 NAMED, 350 UNNAMED, 15 unclaimed).
The 15 trailing bits are HROW word-50 low bits 0-13 in minors
26/27/28, all unmapped in the open DB, and cluster data proves
they are deterministic functions of clock topology, not
version noise: control17 and control22 match the oracle
exactly (0x41cb2/0x10cf/0x8019b7), while BUFG-site, BUFH, and
multi-clock variants move them lawfully. Tie-subset parking is
now evidence-backed: Vivado's own <const0> tie patterns vary
across directives on identical netlists, so they are router
internals, not matchable logic. Seven more sweep jobs are
building (four BOT rows on GCLK0, three TOP rows on GCLK1-3)
to complete the trunk-by-mux matrix, then the segment rule
gets implemented, verified, and committed. No behavior change
(log-only turn).

## Thirty-first run (Nov 9, 2026, Mac, HROW fix landed, 484 to 456)

Cluster tunnel dropped mid-session (socket gone, host
unreachable), so trunk-extension samples G4-G7 and the 2022.1
probe dumps are stranded until the tunnel is restored. Worked
with complete local data instead: the HROW word-50 entry/tap
code is fully determined for trunks 0-3 (12 agreeing samples
across 2017.3, 2022.1, and the oracle, plus directive and row
replicates; OR/XOR/stub-feed decompositions all provably fail,
so the cell table is the derived mapping, gated to its measured
domain). Implemented `clkdist::hrow_codes` plus
`FrameStore::apply_raw_bits`, hooked into the carry e2e
assembly from routed spine data (trunk from spine, tap from the
HROW pip, tile pair cross-checked). Failing-first e2e
assertions on the four oracle word-50 values failed before
(left 0x40000) and pass after. rawdiff: 456 (134 named, 322
unnamed, 15 unclaimed), down exactly the 28 HROW-region bits.
Full xilinx7 suite green except two ff_control_bits failures
proven pre-existing on the clean base (stashed re-run fails
identically; unrelated FFSYNC/ZRST emit assertions). No
behavior change beyond the 28 bits. Next: HCLK word-50
derivation from local samples, then named route bits.

## Thirty-second run (Nov 9, 2026, Mac, leaf/input codes landed, 456 to 412)

Scanned all HCLK tiles for words stable across 8 diverse builds
(2017.3, 2022.1, oracle; single/multi-clock; BOT/TOP loads; BUFG
and BUFH clocks; three directive variants) and found six: the
HCLK_L leaf-buffer pair, the HCLK_CMT_L feed word, and three
HCLK_IOI3 clock-input words, all version-stable and
netlist-stable. Holdouts confirmed the split: input codes hold
for every topology (shared Y9 input path), leaf codes only for
BOT taps (TOP taps use different leaf tiles, unmeasured).
Extended clkdist with leaf_codes (trunks 0-1, BOT R8, gated
elsewhere) and input_codes (fixed full words, single-clock
gate), hooked both into the carry e2e assembly next to the
HROW codes, failing-first assertions failed before and pass
after (all six words now bit-exact). rawdiff: 412 (134 named,
278 unnamed, 15 unclaimed), down exactly the 44 new bits. Full
xilinx7 suite: 111 lib + all integration green except the two
ff_control_bits failures proven pre-existing on the clean base.
No behavior change beyond the 44 bits. Next: per-load HCLK
delay-tap rule (needs load-to-minor mapping), then named route
bits. Cluster tunnel still down; G4-G7 and 2022.1 probe data
stranded.
