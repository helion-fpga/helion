# FFMUX emission lane: blinky emits FFMUXA1.D6 and verifies on route, extract, and cosim

Date: 2026-11-22. Lane branch fm/ultra-bitgen-ffmux, code tip
618e2be2 (merge of W-FLOW 78315041 onto W-CHECK 471bd99d onto
W-CORPUS e3b23142 onto emitter base 15ec6db2). Report commit: 56e13f2c.
Worktree ~/workspace/helion-ultra-bitgen-ffmux. No merges into
fm/ultra-bitstream, fm/zedboard-merge, or master. No em dashes in
this report (verified by grep). Workstream reports:
tools/ffmux-scope/SCOPE.md (502 lines),
tools/ffmux-corpus/FFMUX_CORPUS_REPORT.md (436 lines),
tools/ffmux-check/FFMUX_CHECK_REPORT.md (258 lines),
tools/ffmux-flow/FFMUX_FLOW_REPORT.md (193 lines). Logs:
~/workspace/ffmux-merged-logs/ (merged verification chain),
~/workspace/ffmux-corpus-logs/, ~/workspace/ffmux-check-logs/,
~/workspace/ffmux-flow-logs/.

## Headline

Blinky on xczu3eg now emits FFMUXA1.D6, and the flow-emitted
bitstream passes the US-09 checkers on route, extract, and cosim
with the fixture RTL: route ok (4 PIPs, 3 nets, no violations),
extract ok (1 site modelled, no errors), cosim 400 comparisons,
0 mismatches. The overall verify-bit still exits 1, on decode
only: the strict decoder lists 41 unexplained set bits, all of
them H-1 band bits (1440-1486). Porting the N7 band check into
bitstream_decode.rs is a separate follow-up, not this lane. The
lane's acceptance, as briefed, turns on route, extract, and
cosim, and those three legs are green. This lane does not claim
a fully green verify-bit and does not claim a board-ready blinky:
decode still fails loud, the IOB nets are still skipped, and the
counter still OveruseTimeouts.

## 1. What changed (three workstreams, all merged --no-ff)

- W-SCOPE (a19721a6): tools/ffmux-scope/SCOPE.md (502 lines).
  Found the FF D net is nowhere in pack (no ff_d in
  PackedUltraSlice; extract_cells drops D) but is recoverable
  from design.nets via pin_to_net; the 7-series precedent
  mux_for_ff in helion-xilinx7 emit.rs:1378-1400; segbits
  FFMUXA1 has 6 options, BYP sets only 15_01, D6 only 14_05;
  ff_attr V0=1 is opposite the flow side V1=1.
- W-CORPUS (a06d8d5c) commit aad344d7, merged as e3b23142:
  tools/ffmux-corpus/FFMUX_CORPUS_REPORT.md (436 lines) +
  census.py + segbits_check.py. Census and the BYP vs D6
  decision (section 2).
- W-CHECK (919754c9) commit 656429a7, merged as 471bd99d:
  tools/us09v/ only (code, fixtures, generator, README).
  Flipped ff_attr to V0 = INIT 0 (section 3), adopted
  .FWD/.REV PIP normalization (section 4), re-ran the D6
  variant (section 5). No crates/ file touched.
- W-FLOW (6ae0f044) commit 78315041, merged as 618e2be2:
  exactly 4 files, crates/helion-cli/src/ultra.rs (+297/-46),
  crates/helion-ultrascale/src/fasm_gen.rs (+146),
  crates/helion-ultrascale/src/lib.rs (+3/-1),
  tools/ffmux-flow/FFMUX_FLOW_REPORT.md. No tools/us09v/
  touched in that commit. Emits D6/BYP in the flow and skips
  intra-slice LUT-O to FF-D routing.
- Full lane diff vs base 15ec6db2: 22 files, +2475/-102,
  0 em dashes.

## 2. BYP vs D6 decision (corpus evidence)

Census over all 14 decoded Vivado pairs (478,692 FFMUX lines):
BYP 264,704 (55.3%), D6 126,880 (26.5%), XORIN 45,705
(9.5%), D5 39,480 (8.2%), CY 1,020, F7F8 903. The decisive
question (Q2, 456,354 rows where the FF has an FFMUX line and
the same-letter LUT has an INIT line): D6 with AX undriven
48,316; BYP with AX fabric-fed 244,662; BYP fed by the same
slice's same-letter LUT O exactly 7 rows (0.0015%). D6 total
for the edge is 119,729 (48,316 undriven + 71,413 fed). D6 is
overwhelmingly an FF1 selection (125,057 of 126,880) and D5
an FF2 selection (39,276 of 39,480): the LUT6_2 dual-output
packing, FF1 takes O6, FF2 takes O5.

Decision (coordinator, on the corpus evidence): emit D6 when
the FF D is the same-slice LUT O (same letter); do not route
that edge through the INT to the AX site pin. Emit BYP when
the FF D comes from outside the slice. The 7 own-LUT BYP cases
are all letter D, FF2, and look like a real but vanishingly
rare Vivado corner, not a pattern to follow. Segbits
confirmation: all four CLE tile-type DBs have 96 FFMUX
features each (16 muxes x 6 options); every option sets
exactly one bit and clears five, 0 violations across all 384
features. FFMUXA1: XORIN 13_04, D5 13_05, CY 14_02, F7F8
14_03, D6 14_05, BYP 15_01.

Implementation (W-FLOW): new FfMuxSel enum (D6, Byp),
ffmux_fasm_line, emit_ffmux_fasm next to emit_ff_fasm, one
{site}.FFMUX{letter}{1,2}.{D6|BYP} line per used FF BEL, in
placed-slice order then BEL index ascending. A FF cell missing
from the selection map fails loud. ffmux_selections(design,
packed) recovers the FF D net from design.nets via pin_to_net,
finds the net's LUT-O driver, compares packed slice index and
BEL letter: same slice and same letter means D6; anything else
(other letter, other slice, FF Q, carry output, port, or no D
net) means BYP. Router skip in build_routable_nets: intra-slice
LUT-O to FF-D edges are not fabric nets; a net whose sinks are
all intra-slice is dropped entirely, with a loud hang_diag line
naming the net and the D6 reason. Other sinks on the same net
still route, so the LUT O still reaches the fabric where it is
consumed elsewhere, and the FF D is never double-driven.

Blinky exercises D6 only (same-slice LUT-O to FF-D). BYP
emission is implemented and unit-tested but no flow design in
this lane exercises a BYP leg end to end.

## 3. FF INIT polarity: V0 = INIT 0, V1 = INIT 1 (balance of evidence)

W-CORPUS Q3 and W-CHECK section 1. Used FFs in the 14-pair
corpus are 98.5% V0 (471,517 of 478,692); V1 is 7,175
(1.5%). Exact structural fact: every V0 line in the corpus
sits on a used FF; every unused FF slot reads V1 (the
cleared-bit default). The Xilinx FDRE INIT default is 1'b0,
and HLS designs have the vast majority of registers resetting
to 0; under V0 = INIT 1, 98.5% of used FFs would power up
at 1, which is not plausible. Cross-family analogy: 7-series
prjuray segbits use AFF.ZINI (bit set) for INIT 0, and the
Helion 7-series emitter writes ZINI for INIT 0, validated on
ZedBoard hardware. The bit-level fact (V0 sets the segbits
bit, V1 clears it) is compatible with the value reading
because bit = NOT(INIT). The old checker reading was
circular: ff_attr returned 1 for V0 "pinned by the golden
co-sim tests", and gen_fixtures.py wrote V1 for reg q = 1'b0;
neither is Vivado data.

This is decided on balance of evidence, NOT settled by a
direct Vivado known-INIT comparison. The exact missing datum,
still open: a decoded Vivado bitstream for a design with at
least one FF of independently known INIT 1 (and many of known
INIT 0) on a decoder-supported part (xczu3eg), with the
FF-to-slice mapping recoverable. Nearest miss: spiOverJtag
P-02 on xcku3p, blocked on the missing xcku3p
tilegrid/tileconn/segbits DB. INTERFACES.md item 5 now states
the resolved mapping (V0 = INIT 0, V1 = INIT 1) with the
missing datum named, as balance of evidence not direct proof.

W-CHECK applied the fix on the checker side only: ff_attr in
tools/us09v/us09v/fasmio.py now returns 0 for .V0 and 1 for
.V1, for both INIT and SRVAL; group attributes (CEUSED,
SRUSED, CLKINV, SRINV) keep the opposite convention. The
emitter was NOT flipped. Fixtures updated consistently:
blinky, counter4, carry8-inc FASM files (every *FF.INIT.V1 /
*FF.SRVAL.V1 becomes V0 for registers resetting to 0) and
gen_fixtures.py ff_feats. Regeneration note: blinky and
carry8-inc regenerate byte-identical, but counter4 does NOT
(the route-finder BFS picks a different route for one net);
the checked-in fixtures were restored to the V-flip-only
edit. The generator nondeterminism is pre-existing, flagged,
not fixed here.

## 4. .FWD/.REV bidirectional BYPASS PIP normalization

Corpus incidental finding, now real in the checker: 18,890 PIP
features across the 14 decoded pairs carry a fourth component
(10,389 .FWD, 8,501 .REV), all bidirectional BYPASS PIPs.
The us09v PIP readers matched PIP.<dst>.<src> exactly, so
they missed all of them, likely contributing to undriven-sink
counts. New shared helper parse_pip(feat) in
tools/us09v/us09v/fasmio.py returns the normalized (dst,
src): plain PIP.D.S means S drives D; PIP.D.S.FWD means the
same; PIP.D.S.REV means D drives S, i.e. plain PIP.S.D. The
segbits DB confirms the semantics. Adopted at every PIP read
site: routecheck.py (normalizes, dedupes, so plain and .FWD
spellings on one tile count as one PIP), extract.py driver()
and _wire_consumed(), ports.py trace_pad_to_slice_pin().
Parsed tiles are NOT rewritten; normalization happens at each
read site. New tests: 4 parse_pip unit tests, 3 routecheck
normalization tests, 1 extract .FWD test (8 new tests total,
section 8).

## 5. D6 re-run (W-CHECK, on the old FASM)

The decoded blinky FASM (82 lines, no FFMUX) plus
SLICE_X1Y0.FFMUXA1.D6, fixture blinky RTL (reg q = 1'b0),
200 cycles, seed 1. Log
~/workspace/ffmux-check-logs/us09v-d6-rerun-decoded.log.
Cosim is fixed: 400 comparisons, 0 mismatches (was 400
mismatches). The extracted reg is now 1'b0 and SRVAL 1'b0.
The same holds for the shorter flow-FASM variant (13 lines
plus D6). Overall verify_fasm is still FAIL on that old FASM,
on route only, with violations word-identical to the original
D6 log: the route checker does not count the LUT O pin {c}_O
as a driver, so the A_O net is undriven and two PIP nets are
driverless. That route failure is the pre-existing checker gap
(SCOPE section c); it needs the _slice_output_pins fix, which
belongs to the BYP/route work, not to the D6 polarity fix.
On the NEW flow FASM (section 7) the AX-leg PIPs are gone
and route passes.

## 6. New blinky FASM (12 lines, was 13)

```
SLICE_X1Y0.A6LUT.INIT[63:0] = 0x5555555555555555
SLICE_X1Y0.AFF.INIT.V0
SLICE_X1Y0.AFF.SRVAL.V0
SLICE_X1Y0.FFMUXA1.D6
SLICE_X1Y0.ABCDFF.CEUSED.V0
SLICE_X1Y0.ABCDFF.SRUSED.V0
SLICE_X1Y0.ABCDFF.CLKINV.V0
SLICE_X1Y0.ABCDFF.SRINV.V0
INT_X1Y0.PIP.INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14
INT_X1Y0.PIP.BYPASS_W5.INT_NODE_IMUX_42_INT_OUT1
INT_X1Y0.PIP.INT_NODE_IMUX_48_INT_OUT0.BYPASS_W5
INT_X1Y0.PIP.IMUX_W10.INT_NODE_IMUX_48_INT_OUT0
```

13 lines became 12: the added FFMUXA1.D6 line (+1) is
outweighed by the two AX-leg PIPs that disappeared (-2).
The 4 remaining PIPs are the AQ feedback leg (LUT input A1).
The flow log confirms the skip: hang_diag ultra route skip
net=q_d reason=intra-slice LUT-O to same-letter FF-D (D6, no
fabric route), route nets=1 iters=1 overused=0,
route-to-pip pips=4.

## 7. Flow run and decode round-trip

Command (release binary, tileconn env
HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp,
HELION_TILECONN_PART=xczu3eg-sbva484-1-e):

```
./target/release/helion impl examples/blinky.sv --part xczu3eg
```

Exit 0. Bitstream 5,982,532 bytes (same as the emitter-lane
blinky); 14,952 FDRI frames; IDCODE 0x04a42093 present;
STA WNS_PS=9650, estimate=1; parse_back_lines=567641. Log
~/workspace/ffmux-merged-logs/flow-blinky-merged.log.

Decode: exit 1 with exactly 41 unexplained set bits, every
one at absolute frame bits 1440-1486, inside the H-1 band
(1440-1487), across 6 FARs; the decoder does round-trip
SLICE_X1Y0.FFMUXA1.D6 (the D6 bit itself is explained).
The count moved from 38 (emitter lane) to 41 because the H-1
band code is computed from frame contents: dropping the 2
AX-leg PIP bits and adding the FFMUX bit changed which band
bits are set. The invariant is not the count but that every
unexplained bit is a band bit and the band check is Exact on
every frame (14,952/14,952 in the regression test). The test
now pins 41 with that explanation in its doc comment. Log
~/workspace/ffmux-merged-logs/decode-blinky-merged.log.

## 8. Suites

Baselines (emitter tip 15ec6db2): ultrascale 423/0/1, CLI
70/0/1, checkers 75 OK.

- helion-ultrascale: 426 passed, 0 failed, 1 ignored
  (baseline 423/0/1 + 3 new fasm_gen FFMUX tests). Log
  ~/workspace/ffmux-merged-logs/test-ultrascale-merged.log,
  ULTRA_EXIT:0, 31 result lines.
- helion-cli: 72 passed, 0 failed, 1 ignored (baseline
  70/0/1 + 2 new ffmux_selections tests). Log
  ~/workspace/ffmux-merged-logs/test-cli-merged.log,
  CLI2_EXIT:0 after the correction in section 9.
- US-09 checkers: 83 tests, OK, 154.488 s (baseline 75 + 8
  new: 4 parse_pip unit tests, 3 routecheck FWD/REV tests,
  1 extract .FWD test). Log
  ~/workspace/ffmux-merged-logs/test-checkers-merged.log,
  CHECKERS_EXIT:0.
- Release build: exit 0 (BUILD_EXIT:0 after the correction
  in section 9). Log
  ~/workspace/ffmux-merged-logs/build-merged.log.

Regression test updates (W-FLOW): blinky_flow_emits_bitstream_
on_xczu3eg expected array 13 to 12 lines, count assert 13 to
12, unexplained-bit assert 38 to 41, doc comment updated.
Same pins otherwise: IDCODE 0x04a42093, 14,952 FDRI frames,
band Exact on all frames, decoder round-trips every flow
line. US-09 verify-bit is still deliberately NOT asserted:
decode still exits 1 on the band bits. New unit tests (5):
fasm_gen gains ffmux_lines_use_letter_and_index_from_bel,
ffmux_missing_selection_fails_loud,
ffmux_names_cover_all_sixteen; ultra.rs gains
ffmux_selections_same_letter_lut_is_d6 and
ffmux_selections_other_sources_are_byp. The existing
io_and_clock_nets_skip_loudly_not_error was updated: the
fixture q net is now intra-slice, so zero nets route.

## 9. Merged verification chain incidents (all recorded, none hidden)

The first merged chain (launched 09:39:50 UTC) was killed by
a runtime restart at about 09:42 UTC, roughly 2.5 minutes in,
before any step completed (chain.out held only ULTRA_START;
the ultrascale log ended mid-compile, 0 test-result lines).
The partial log is preserved as
test-ultrascale-merged-partial1.log. The first tail
coordinator did not relaunch it, per its brief; the
replacement tail coordinator relaunched from scratch at
09:46:18 UTC, and that chain ran to CHAIN_DONE 10:23:44 UTC
(38 minutes). Zero steps had completed in the killed chain,
so the relaunch redid nothing that finished.

Two defects in the chain script itself, both found and fixed
by the replacement tail coordinator (chain.sh in
~/workspace/ffmux-merged-logs/, fixed version kept there
with chain.sh.orig preserved):

1. The chain exports HELION_TILECONN_DB_ROOT /
   HELION_TILECONN_PART globally, so the CLI suite ran WITH
   the tileconn env and failed 2 tests
   (blinky_stops_at_route_without_pin_wire_map,
   counter_stops_at_route_without_pin_wire_map), which assume
   no pin-wire map exists. That is the known pre-existing
   test/env interaction, not a regression. Fix: the CLI step
   now runs with the env unset (matching the baseline
   condition). The with-env run is preserved as
   test-cli-merged-withenv.log.
2. The build step used package name helion-bitgen, which does
   not exist (cargo: "a package with a similar name exists:
   helion-bits"). Fix: build -p helion-cli -p
   helion-ultrascale (the package that owns the helion-bitgen
   binary). The bad run is preserved as
   build-merged-badpkg.log.

A third incident on the first CLI rerun: it failed 1 test
(blinky_stops_at_route_without_pin_wire_map, "bitstream file
exists despite the ROUTE failure") because a stale
/tmp/helion-us09-flow-blinky/blinky.xcku3p.bit (16,585,732
bytes, mtime 10:21) was left by the with-env chain run, and
the test asserts no bitstream exists at that fixed /tmp path
after a ROUTE failure. Removing the stale files fixed it;
the clean rerun is 72/0/1, CLI2_EXIT:0. This is the same
stale-file class the flow report already documented. The
fixed /tmp path shared across runs is a pre-existing test
hygiene issue, not a lane regression. chain.out carries a
CHAIN_CORRECTION line recording all three repairs and the
final counts.

## 10. US-09 verify-bit on the emitted bitstream

Command: python3 -m us09v verify-bit --bit
flow-blinky/blinky.xczu3eg.bit --design blinky, 200 cycles.
Logs: ~/workspace/ffmux-merged-logs/verify-blinky-merged.log
(raw RTL) and verify-blinky-fixture-merged.log (fixture RTL).

Fixture RTL (acceptance variant):

```
blinky: FAIL
  [FAIL] decode: exit 1: helion-bitgen: decode failed: 41 set bits no feature explains (listed above)
  [ok] parse: 2 tiles, 81 features
  [ok] ports: 1 inputs, 1 outputs from port map
  [ok] place: 1 slices, 0 carry slices; no violations
  [ok] route: 4 PIPs, 3 nets; no violations
  [ok] extract: 1 sites modelled; no errors; 18 warnings
  [ok] cosim: 400 comparisons, 0 mismatches, 0 RTL unknowns, 0 net unknowns; no mismatches
```

Route, extract, and cosim all pass. The only failing leg is
decode, on the 41 band bits (section 7), which is the N7
port follow-up, not this lane. Overall ok is False because
decode fails; the lane does not claim an overall PASS.

Raw RTL variant: identical except cosim is 0 comparisons,
400 RTL unknowns, because examples/blinky.sv declares
logic q with no initial value (pre-existing, same as the
emitter lane). Route and extract pass there too.

Note the route result precisely: route passes on the NEW flow
FASM because the AX-leg PIPs are gone. The route checker's
LUT-O-driver gap (section 5) still stands for FASMs that
still carry AX-leg PIPs; it needs the _slice_output_pins fix.

## 11. Counter (stretch)

Exit 1, no .bit, exact known failure unchanged: ROUTE
OveruseTimeout, net cnt_1, wire 36, tile (1,0), 3 nets, 50
iters. Log ~/workspace/ffmux-merged-logs/flow-counter-merged.log.

## 12. Carried caveats and gaps

- Decode still exits 1 on band-only streams: port the N7 band
  check into bitstream_decode.rs (follow-up lane, blocker 1).
- Route checker _slice_output_pins fix outstanding (section 5):
  a LUT O pin is not counted as a driver. Needed for
  BYP-through-fabric paths; the D6 blinky route passes, but
  any FASM with AX-leg PIPs still fails route on A_O.
- BYP emission is implemented and unit-tested but not
  exercised end to end by any flow design in this lane; blinky
  exercises D6 only. A BYP-leg flow design is the next
  routing proof.
- FF INIT polarity stands on balance of evidence, not direct
  proof (section 3); the missing known-INIT Vivado datum is
  still open, nearest miss P-02 blocked on the missing xcku3p
  DB.
- gen_fixtures.py counter4 regeneration nondeterminism
  (pre-existing, section 3).
- Counter ROUTE OveruseTimeout unchanged; IOB cells still
  have no placed site (2 nets skipped per run); no documented
  xczu3eg timing; INT_X2+ still unproven (set_int_columns
  not wired); flags A-CTRL and A-F7F8 still unresolved, need
  a CTO ruling.

## Next blockers, in order

1. Port the N7 band check into bitstream_decode.rs (decode
   exit 1 on band-only streams).
2. Route checker _slice_output_pins fix (LUT O as driver).
3. A BYP-leg flow design end to end (BYP emission is
   unexercised in the flow).
4. Counter ROUTE OveruseTimeout congestion.
5. IOB placement (2 nets skipped per run).
6. Wire set_int_columns for INT_X2+ on xczu3eg.
7. The missing known-INIT Vivado datum (P-02 blocked on the
   missing xcku3p tilegrid/tileconn/segbits DB).
8. Documented xczu3eg timing.
