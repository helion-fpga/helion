# FFMUX Corpus Report

Worker lane W-CORPUS for the FFMUX emission lane (UltraScale+ grind).
Branch: `fm/ultra-bitgen-ffmux-corpus`, base `15ec6db2`.
Date: 2026-11-22. Evidence only: no production code was changed.

## Recommendation

**Emit D6 when the FF D is the same-slice LUT O (same letter). Do not emit
BYP and do not route the LUT O through the INT to the AX site pin for that
edge.** When the FF D comes from outside the slice (another site's Q, MUX,
or O, or a port), emit BYP and route to AX as the flow already does.

**Do not flip the emitter's FF INIT polarity. The corpus favors V0 = INIT 0,
and the D6 cosim inversion is most plausibly a checker-side polarity error
in us09v `ff_attr`, not an emitter bug.** Recommended action: change
`ff_attr` to read V0 as INIT 0 and V1 as INIT 1, then re-run the D6 cosim.
The polarity verdict is NOT settled by a direct known-INIT comparison (no
such pair exists yet); the exact missing datum is named in Q3.

Why D6, in one paragraph: across all 14 decoded Vivado pairs, Vivado
encodes a same-slice LUT-O to FF-D edge as D6 in 119,729 cases, and it
encodes the same edge as BYP with a fabric route from the same LUT O in
exactly 7 cases out of 456,354 (0.0015%). BYP is the shape for D coming
from elsewhere: 244,669 BYP rows have the AX net fabric-fed, and their
drivers are overwhelmingly other slices' Q/O/MUX or the same slice's Q,
never the same slice's LUT O. The D6 shape with an undriven AX pin (48,316
cases) is the clean same-slice encoding; the flow should emit that.

## Sources and method

Pairs: the 14 decoded pair FASMs under
`~/workspace/us09-checkers-logs/pairs/`, all xczu3eg-sbva484-1-e:

| Pair | Design | FFMUX lines |
|------|--------|-------------|
| P-11 | resizer (Ultra96) | 8,024 |
| P-13 | BNN-PYNQ CNV | 40,524 |
| P-14 | BNN-PYNQ CNV | 50,743 |
| P-15 | BNN-PYNQ CNV | 47,854 |
| P-16 | BNN-PYNQ LFC | 30,948 |
| P-17 | BNN-PYNQ LFC | 36,811 |
| P-23 | FINN | 33,220 |
| P-28 | PYNQ-ComputerVision xv2Filter2DDilate | 16,443 |
| P-40 | FINN | 39,826 |
| P-41 | FINN | 39,980 |
| P-42 | FINN | 19,559 |
| P-43 | FINN | 47,615 |
| P-44 | FINN | 48,218 |
| P-45 | FINN | 18,927 |
| **Total** | | **478,692** |

P-11's `P-11/decoded.fasm` is byte-identical to
`P-11_resizer_ultra96.decoded.fasm` (md5
`8aa6574aa330f62128ffac50b3e411f6`) and was counted once. Every pair has
exactly 8,820 slices with features, 3,600 of them SLICEM, and exactly
141,120 FF slots (8,820 x 16), one INIT line per slot.

Census script: `tools/ffmux-corpus/census.py`. It parses each FASM with
`us09v.fasmio.parse_fasm_file`, builds the `Db` + `Fabric` + `Extractor`
from `tools/us09v/us09v/` (DB root `~/workspace/prjuray-db-ref/zynqusp`,
part `xczu3eg-sbva484-1-e`), finds the AX pin wire with
`Fabric.site_pin_wire`, its node with tileconn stitching (`Fabric.node`),
collects active PIPs feeding any wire of that node, and resolves the
ultimate driver with `Extractor.driver`. Sibling-FF selection, real carry
use (`Extractor._carry_used`), wide-mux use, and OUTMUX selection are
recorded per row. Full output:
`~/workspace/ffmux-corpus-logs/census-all14.json` (456,354 Q2 rows),
`~/workspace/ffmux-corpus-logs/census-all14.log` (exit 0, about 299 s).
Segbits check: `tools/ffmux-corpus/segbits_check.py`.

## Caveats on the decoded corpus (read before trusting any count)

1. Every pair decodes with exit 1 and 70,801 (P-11) to 232,023 (P-14)
   unexplained bits, and the US-09 route checker reports hundreds of
   thousands of undriven-sink violations per pair. Decoded routing is
   incomplete. Counts below are counts of what the decoder recovered,
   not counts of what Vivado placed.
2. `n_slices=8820` and `n_slicem=3600` are identical in every pair
   because the decoder emits default features (INIT/SRVAL/DI/CARRY_TYPE)
   for every slice in the decoded region. "Used FF" in this report means
   a FF with an FFMUX line, which is the only reliable use marker.
3. FWD/REV PIP suffixes: 18,890 PIP features across the 14 pairs carry
   a fourth component (10,389 `.FWD`, 8,501 `.REV`). All are
   bidirectional BYPASS PIPs (only 4 such PIPs exist in the INT tile
   type, all involving BYPASS_E8/E14/W8). Segbits confirms direction
   semantics, e.g. `INT.PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0.FWD`
   sets `71_13 74_24` while `.REV` sets `71_13` and clears `74_24`.
   The us09v `PIP_RE` in `routecheck.py` and the exact-match PIP lookup
   in `extract.py` `driver()` both miss these, so the US-09 checkers
   systematically miss FWD/REV PIPs. This likely contributes to their
   large undriven-sink counts. The census normalizes them in memory
   only (FWD becomes `PIP.D.S`, REV becomes `PIP.S.D`). Verification on
   P-11: 0 (tile, dst, src) pairs appear in both plain and suffixed
   form and 0 pairs have both FWD and REV, so there is no double
   counting. The fix moved P-11 BYP-fed from 3,404 to 3,534 and
   BYP-undriven from 332 to 202. The US-09 lane should adopt the same
   normalization.
4. `CARRY8.DI<i>.<letter>X` lines are complement/default features
   (`!12_03` etc., present on 8,756 of 8,820 slices in P-11) and do NOT
   indicate real carry use. An early census version used them as a
   carry flag and was corrected to use `Extractor._carry_used` (CY or
   XORIN selection, CI/CI_TOP selections, DI.DI selections, consumed
   COUT).
5. LUT INIT is never zero in the decoded FASMs (0 occurrences of
   `0x0000000000000000` among 69,930 LUT INIT lines in P-11 and 67,248
   in P-13), so "LUT INIT present" means a real nonzero LUT function.
   Note the recurring value `0x8000000080000000` (5-input AND) on
   apparently unconsumed LUTs (e.g. SLICE_X0Y24 letters E/F/G in P-11);
   it is part of the same unexplained pattern as the D6-fed AX routes
   in Q2.

## Q1: FFMUX selection census

478,692 FFMUX lines across the 14 pairs.

By selection:

| Selection | Count | Share |
|-----------|-------|-------|
| BYP | 264,704 | 55.3% |
| D6 | 126,880 | 26.5% |
| XORIN | 45,705 | 9.5% |
| D5 | 39,480 | 8.2% |
| CY | 1,020 | 0.2% |
| F7F8 | 903 | 0.2% |

By FF index (FF1 vs FF2) the split is sharp: D6 is overwhelmingly an
FF1 selection (125,057 of 126,880) and D5 is overwhelmingly an FF2
selection (39,276 of 39,480). That is the LUT6_2 dual-output packing:
FF1 takes O6, FF2 takes O5. FF1 total 286,886 (BYP 122,275, D6 125,057,
XORIN 37,428, D5 204, CY 1,020, F7F8 902). FF2 total 191,806 (BYP
142,429, D5 39,276, D6 1,823, XORIN 8,277, F7F8 1, CY 0).

By letter and selection (letter: BYP / CY / D5 / D6 / F7F8 / XORIN):

| Letter | BYP | CY | D5 | D6 | F7F8 | XORIN |
|--------|-----|----|----|----|-----|-------|
| A | 16,322 | 1 | 3,580 | 11,764 | 53 | 5,936 |
| B | 14,388 | 20 | 5,944 | 19,471 | 42 | 5,994 |
| C | 18,002 | 59 | 8,603 | 26,436 | 475 | 5,867 |
| D | 32,556 | 283 | 10,583 | 32,158 | 113 | 5,697 |
| E | 44,577 | 3 | 1,525 | 4,085 | 0 | 5,675 |
| F | 38,747 | 68 | 1,987 | 6,850 | 14 | 5,679 |
| G | 44,295 | 0 | 3,117 | 10,677 | 133 | 5,526 |
| H | 55,817 | 586 | 4,141 | 15,439 | 73 | 5,331 |

The letter asymmetry is strong: E/F/G/H letters are BYP-heavy and
A-D letters are D6/D5/XORIN-heavy. Per-pair selection counts (BYP, D6,
D5, XORIN, CY, F7F8): P-11: 3,942, 2,230, 1,047, 795, 5, 5. P-13:
18,930, 10,921, 2,651, 7,913, 106, 3. P-14: 27,930, 12,634, 2,113,
7,872, 191, 3. P-15: 27,781, 10,705, 2,120, 6,920, 115, 213. P-16:
10,512, 11,636, 3,103, 5,674, 20, 3. P-17: 14,209, 14,388, 1,883,
5,673, 276, 382. P-23: 18,778, 8,511, 3,649, 2,264, 9, 9. P-28: 8,596,
4,664, 2,360, 582, 12, 229. P-40: 25,510, 8,184, 4,129, 1,948, 41, 14.
P-41: 25,572, 8,222, 4,182, 1,948, 41, 15. P-42: 9,757, 5,894, 2,866,
1,035, 2, 5. P-43: 31,428, 11,755, 3,193, 1,131, 100, 8. P-44: 31,754,
11,979, 3,281, 1,099, 100, 5. P-45: 10,005, 5,157, 2,903, 851, 2, 9.

## Q2: the decisive question

Population: every (slice, letter, FF index) row where the FF has an
FFMUX line AND the same-letter LUT has an INIT line. 456,354 rows across
the 14 pairs. For each row, "AX fed" means at least one active PIP
feeds a wire of the node containing that slice's AX site pin; "driver"
is the ultimate site driver resolved by `Extractor.driver`.

| Bucket | Count |
|--------|-------|
| (a) D6 with AX undriven | 48,316 |
| (b) BYP with AX fabric-fed | 244,662 |
| (b') BYP with AX fed by the same slice's same-letter LUT O | 7 |
| BYP with AX undriven | 11,804 |
| D6 with AX fed (explained below) | 71,413 |
| D5 fed / undriven | 19,440 / 13,087 |
| XORIN fed / undriven | 43,870 / 1,835 |
| CY fed / undriven | 654 / 366 |
| F7F8 fed / undriven | 776 / 124 |
| **Total** | **456,354** |

D6 total is 119,729 (48,316 undriven + 71,413 fed). The 7 own-LUT
cases are the only instances in the whole corpus of Vivado routing a
same-slice LUT O to its own FF through the fabric and BYP: 7 of
456,354 rows, 0.0015%. 28,091 of the BYP-fed rows do not resolve to a
driver, so the 7 is a lower bound; but the same-slice-LUT rate among
resolved BYP-fed rows is 7 in 216,578 (0.003%), so even a 100x
enrichment in the unresolved rows would keep the share under 0.04%.
Vivado essentially never uses the BYP shape for a same-slice LUT to FF
edge.

Per-pair rollup (D6 undriven / D6 sibling-BYP / D6 carry / D6 wide /
D6 unexplained / BYP fed / BYP undriven / BYP own-LUT):

| Pair | D6 undriven | D6 sibling | D6 carry | D6 wide | D6 unexplained | BYP fed | BYP undriven | BYP own-LUT |
|------|------------|-----------|----------|---------|----------------|---------|--------------|-------------|
| P-11 | 822 | 209 | 0 | 23 | 1,139 | 3,534 | 202 | 1 |
| P-13 | 5,215 | 1,252 | 1 | 63 | 4,102 | 16,844 | 1,365 | 1 |
| P-14 | 4,609 | 2,384 | 6 | 100 | 5,247 | 25,065 | 1,893 | 0 |
| P-15 | 4,250 | 1,613 | 5 | 111 | 4,310 | 25,222 | 1,540 | 0 |
| P-16 | 5,648 | 784 | 3 | 9 | 4,952 | 9,354 | 755 | 0 |
| P-17 | 5,446 | 927 | 3 | 158 | 7,614 | 12,902 | 852 | 0 |
| P-23 | 3,104 | 560 | 0 | 84 | 3,993 | 17,554 | 706 | 0 |
| P-28 | 1,667 | 616 | 0 | 60 | 2,070 | 7,594 | 709 | 0 |
| P-40 | 2,762 | 675 | 0 | 15 | 3,834 | 23,672 | 991 | 3 |
| P-41 | 2,934 | 716 | 0 | 14 | 3,660 | 23,382 | 950 | 1 |
| P-42 | 1,906 | 438 | 0 | 1 | 2,843 | 9,028 | 428 | 1 |
| P-43 | 4,182 | 507 | 0 | 9 | 6,351 | 30,490 | 553 | 0 |
| P-44 | 4,026 | 471 | 0 | 7 | 6,769 | 30,842 | 547 | 0 |
| P-45 | 1,745 | 292 | 0 | 7 | 2,406 | 9,179 | 313 | 0 |

The pattern is stable across all 14 pairs. SLICEL-only (291,141 rows):
BYP fed 153,999; D6 unexplained 37,057; D6 undriven 32,095; XORIN fed
28,421; D5 fed 12,491 / undriven 9,224; BYP undriven 7,723; D6
sibling-BYP 7,155; D6 wide 410; D6 carry 9; BYP own-LUT 6. SLICEM-only
(165,213 rows): BYP fed 90,663; D6 unexplained 22,233; D6 undriven
16,221; XORIN fed 15,449; D5 fed 6,949 / undriven 3,863; D6 sibling-BYP
4,289; BYP undriven 4,081; BYP own-LUT 1.

(FFMUX1, FFMUX2) per (slice, letter) with LUT present, 307,316 pairs:
(BYP, BYP) 96,359; (D6, None) 74,759; (XORIN, None) 28,747; (D6, D5)
26,771; (None, BYP) 25,092; (BYP, None) 21,175; (D6, BYP) 16,261;
(XORIN, XORIN) 8,240; (None, D5) 5,409; (D6, D6) only 115; (BYP, D6)
461. Two FFs almost never share one LUT O6; the LUT6_2 packs O6 to
FF1 (D6) and O5 to FF2 (D5) instead. The (D6, BYP) vs (BYP, D6)
asymmetry (16,261 vs 461) shows FF1 preferentially takes the LUT O
while FF2 takes fabric D.

BYP-fed rows: driver kinds are other-slice Q 81,387; other-slice O
49,424; other-slice MUX 44,304; same-slice Q 33,913; undriven 28,091;
same-slice O 5,387; same-slice MUX 1,932; unmodelled 231. In most
BYP-fed rows the same-letter LUT O does NOT exit via OUTMUX (None in
204,673 of 244,669, 84%; D6 20,476; D5 9,252; F7F8 8,915; XORIN 1,173;
CY 180), so in most BYP cases the same-letter LUT O is consumed
internally or its exit was not recovered by the decoder; that does not
affect the FFMUX decision, because the FF D in those rows is the fabric
signal, not the LUT O.

The 7 own-LUT cases share one pattern: all letter D, all FF2
(FFMUXD2.BYP), sibling FFMUXD1 absent, OUTMUXD D5 (one F7F8), a
single BYPASS PIP feeding DX, driver the same slice's DMUX. Example:
P-11 SLICE_X8Y43 D2, FFMUXD2.BYP, DLUT INIT 0xfdfdfdfdc4c5f4f5, PIP
`INT_X5Y43.PIP.BYPASS_E3.INT_NODE_IMUX_22_INT_OUT0`, driver site
SLICE_X8Y43.DMUX, OUTMUXD.D5. The other six: P-13 SLICE_X32Y122 D2
(wide mux used), P-40 SLICE_X10Y31 D2, P-40 SLICE_X15Y169 D2 (wide),
P-40 SLICE_X3Y75 D2, P-41 SLICE_X1Y30 D2 (SLICEM), P-42 SLICE_X11Y18
D2. This looks like a real but vanishingly rare Vivado corner, not a
pattern the lane should follow.

Concrete examples for the three buckets:

(a) D6 with AX undriven. P-13 SLICE_X0Y11 letter D, FFMUXD1.D6, DLUT
INIT 0xcacacacaff0ff000, 0 PIPs into DX, driver undriven, sibling
FFMUXD2.D5. Also P-13 SLICE_X0Y12 letter A, FFMUXA1.D6, ALUT INIT
0xaaf0aaf0ccffcc00; P-13 SLICE_X0Y13 letter A, FFMUXA1.D6, ALUT INIT
0xaaaaf0f0ccccff00.

(b) BYP with AX fabric-fed. P-13 SLICE_X0Y24 letter E num 1 and 2,
FFMUXE1.BYP and FFMUXE2.BYP, ELUT INIT 0x8000000080000000, PIP
`INT_X0Y24.PIP.BYPASS_E8.INT_NODE_IMUX_19_INT_OUT1`, driver site
SLICE_X4Y22.AQ2, sibling BYP. P-13 SLICE_X0Y24 letter F num 1,
FFMUXF1.BYP, PIP
`INT_X0Y24.PIP.BYPASS_E9.INT_NODE_IMUX_10_INT_OUT0`, driver site
SLICE_X5Y20.CQ.

(c) D6 with AX fed, unexplained (see open questions). P-11 SLICE_X0Y24
letter D, FFMUXD1.D6, DLUT INIT 0xcccccccc00000000, one PIP
`INT_X0Y24.PIP.BYPASS_E3.INT_NODE_IMUX_22_INT_OUT0` feeding DX, driver
site SLICE_X0Y24.CQ (the same slice's C FF output), sibling FFMUXD2
absent, carry not used, wide mux not used.

D6 with AX fed, explained sub-buckets: sibling FF uses BYP 11,444
(the AX net feeds the sibling FF, and the D6 FF's AX pin is a tap on
that same net); wide mux uses AX 661 (F7F8 path); carry used in slice
18.

## Q3: FF INIT census and polarity

All 1,975,680 FF slots (14 pairs x 141,120): V0 471,517 (23.9%), V1
1,504,163 (76.1%). Used FFs only (478,692, exactly the FFMUX count):
V0 471,517 (98.5%), V1 7,175 (1.5%). Exact structural fact: every V0
line in the corpus sits on a used FF, in all 14 pairs; every unused FF
slot reads V1 (the cleared-bit default). Per-pair used-FF V0/V1: P-11
7,844/180 (97.8% V0). P-13 39,873/651 (98.4%). P-14 50,073/670
(98.7%). P-15 47,131/723 (98.5%). P-16 30,703/245 (99.2%). P-17
36,566/245 (99.3%). P-23 32,551/669 (98.0%). P-28 15,797/646 (96.1%).
P-40 39,140/686 (98.3%). P-41 39,313/667 (98.3%). P-42 19,116/443
(97.7%). P-43 47,157/458 (99.0%). P-44 47,756/462 (99.0%). P-45
18,497/430 (97.7%). SRVAL all slots: V0 472,544, V1 1,503,136, the
same shape as INIT, consistent with SRVAL equal to INIT for most FFs.

Polarity verdict: **NOT RESOLVED by a known-INIT pair.** None of the
14 pairs has a per-FF known INIT. P-11 resizer, P-13 to P-17 BNN, P-23
and P-40 to P-45 FINN, and P-28 PYNQ-ComputerVision are all
HLS/FINN-generated designs with no checked-in RTL; their .hwh files
carry no FF INITs. The blinky/counter goldens are Helion's own
designs with known INIT (blinky `q` is `1'b0`), but their FASM is
Helion-emitted, not Vivado-generated, so they cannot settle what
Vivado's V0/V1 means.

Balance of evidence, which strongly favors **V0 = INIT 0** (the
emitter's reading) over V0 = INIT 1 (us09v `ff_attr`):

1. Used FFs are 98.5% V0. The Xilinx FDRE INIT default is 1'b0, and
   HLS designs have the vast majority of registers resetting to 0.
   Under V0 = INIT 1, 98.5% of used FFs would power up at 1, which
   contradicts the FDRE default and is not plausible for these designs.
   Under V0 = INIT 0, the 1.5% V1 are the registers with an explicit
   INIT 1 (one-hot FSM states, set-dominant registers), a plausible
   fraction.
2. Cross-family analogy: 7-series prjuray segbits use `AFF.ZINI`
   (bit set) for INIT 0, and the Helion 7-series emitter writes ZINI
   for INIT 0 (`crates/helion-xilinx7/src/emit.rs`: "ZINI: power-on
   value 0 (absent: INIT=1)"), validated on hardware via the ZedBoard
   flow. The UltraScale+ convention (V0 sets the bit, V1 clears it)
   matches the same bit-set-means-INIT-0 pattern.
3. The internal positions that disagree are both Helion-authored and
   circular: `ff_attr` in `tools/us09v/us09v/fasmio.py` returns 1 for
   V0 ("V0 sets the bit, V1 clears it... pinned by the golden co-sim
   tests"), and `gen_fixtures.py` writes V1 for `reg q = 1'b0`. The
   "pinning" is the fixture generator agreeing with `ff_attr`; neither
   is Vivado data. The emitter's `translate_ff_valued_fasm` in
   `crates/helion-ultrascale/src/bin/helion-bitgen.rs` treats the V
   suffix as the value (V0 = INIT 0). The D6 cosim result (all 400
   comparisons inverted) is exactly what you get when the emitter
   writes V0 for INIT 0 and the checker reads V0 as INIT 1. The
   corpus says the checker is wrong, not the emitter.

The exact missing datum: a decoded Vivado bitstream for a design with
at least one FF of independently known INIT 1 (and many of known
INIT 0) on a decoder-supported part (xczu3eg), with the FF-to-slice
mapping recoverable. The strongest near-miss on disk: spiOverJtag
P-02, whose .bit (`d2.bit`) is in
`~/workspace/ultra-bitgen-crew/verification-evidence/`, design
spiOverJtag, part xcku3p-ffvb676-2-e, Vivado 2025.2.1, 2026/06/08.
Its RTL was read from GitHub raw on 2026-11-22 (`xilinx_spiOverJtag.v`,
`spiOverJtag_core.v`): plain Verilog, no explicit initial values and
no initial blocks (registers header 16b, header1 7b, hdr_cnt 4b,
jtag_state 3b, ver_cnt 7b, ver_shft 40b, ver_state 3b, csn, sdi_dq0;
jtag_state resets to IDLE 000, csn to 1, sdi_dq0 to 0 via async reset,
which sets SRVAL not INIT), so every FF is INIT 0 by the FDRE default.
A decoded P-02 showing used FFs at ~100% V0 would settle the polarity
statistically with no per-FF mapping needed. Decoding it is BLOCKED:
no xcku3p tilegrid/tileconn/segbits DB exists on disk
(`~/workspace/prjuray-db-ref/zynqusp/` holds only xczu3eg-sbva484-1-e
and xczu3eg-sfvc784-1-e; `~/workspace/ultra-bitgen-crew/harness/
reference/xcku3p.json` is a device description, not tileconn). Either
build or fetch that DB, or place a known-INIT design on xczu3eg.

Note for the lane: `INTERFACES.md` (FASM text section, item 5) states
the value-to-feature mapping is unverified by any on-disk source and
FF INIT stays BLOCKED on a polarity experiment. This report does not
close that item by direct proof; it closes it on the balance of
evidence and names the experiment that would.

## Q4: segbits confirmation

Source: `crates/helion-ultrascale/data/segbits_*.db` on this branch
(ASCII text, prjuray-db CC0). All four CLE tile-type DBs (CLEL_L,
CLEL_R, CLEM, CLEM_R) have 96 FFMUX features each: 16 muxes (A-H x
1/2) x 6 options (BYP, CY, D5, D6, F7F8, XORIN). Every option sets
exactly one bit and clears five: 0 violations across all 384
features. The critic's finding is confirmed. Reproduce with
`python3 tools/ffmux-corpus/segbits_check.py`.

FFMUXA1 six bits are frames 13_04, 13_05, 14_02, 14_03, 14_05, 15_01:

| Option | Sets |
|--------|------|
| XORIN | 13_04 |
| D5 | 13_05 |
| CY | 14_02 |
| F7F8 | 14_03 |
| D6 | 14_05 |
| BYP | 15_01 |

FFMUXA2 uses a different six bits (13_02, 13_03, 14_04, 15_02, 15_03,
15_04): D5 sets 13_02; F7F8 sets 13_03; BYP sets 14_04; CY sets 15_02;
D6 sets 15_03; XORIN sets 15_04. Identical across all four tile types,
e.g. `CLEM.FFMUXA1.D6!13_04!13_05!14_02!14_03 14_05!15_01` and
`CLEM.FFMUXA1.BYP!13_04!13_05!14_02!14_03!14_05 15_01`.

FF INIT bits (frame 12, one bit per FF, identical across all four tile
types, V0 sets the bit, V1 clears it): AFF 12_02, BFF 12_04, CFF
12_08, DFF 12_10, EFF 12_26, FFF 12_28, GFF 12_32, HFF 12_34, AFF2
12_14, BFF2 12_16, CFF2 12_20, DFF2 12_22, EFF2 12_38, FFF2 12_40,
GFF2 12_44, HFF2 12_46. SRVAL: 32 features, same pattern on adjacent
bits (AFF SRVAL V0 12_01, BFF 12_05, AFF2 12_13). 64 INIT/SRVAL
features per DB, all single-bit.

## Open questions (not affecting the recommendation)

1. D6-fed residual, 59,290 rows (49.5% of all D6 rows). The FF uses
   D6, but a single PIP (always exactly one, always a BYPASS or
   BOUNCE PIP) feeds the AX pin from a real signal: other-slice Q
   28,763; same-slice Q 12,738; other-slice O 6,963; other-slice MUX
   5,330; undriven 2,669; same-slice O 2,350; same-slice MUX 415;
   unmodelled 62. Sibling selection is None in 43,009, D5 in 16,157,
   D6 in 124. No carry, no wide mux. The routed net has no other load
   in the node (the node is truncated at the AX pin; the tileconn
   stitch DX to BYPASS_E3 exists in the 81,640-entry public map but
   the onward BYPASS_E3 to BYPASS_L3 stitch does not apply at these
   grid locations). Hypotheses: Vivado routes to unselected mux inputs
   in these HLS designs; decoder over-emission of the single PIP;
   tileconn stitching joining nets Vivado treats as distinct. The
   FFMUX selection still defines the FF D source, so this does not
   change the D6 recommendation.
2. BYP-undriven residual, 11,804 rows (4.6% of BYP rows). Consistent
   with decode gaps (70k-230k unexplained bits per pair) and
   fabric-model gaps; not evidence that Vivado leaves BYP FFs
   undriven.
3. (BYP, BYP) rows where the same-letter LUT O has no OUTMUX exit
   (84% of BYP-fed rows): likely decoder/fabric gaps in OUTMUX
   recovery or LUTRAM-mode semantics. Does not affect the
   recommendation.
4. The letter asymmetry (E-H BYP-heavy, A-D D6-heavy) is reported but
   not explained here.

## Helion's own blinky FASM, for reference

`~/workspace/emitter-xczu3eg-logs/flow2/blinky-decoded.fasm` has 0
FFMUX lines and 17 INIT lines; it writes `AFF.INIT.V0` for `q`
(INIT 0) and `AFF2.INIT.V1` for the unused FF2. The flow's wire model
routes LUT-O to FF-D through the INT to the AX site pin (the BYP
shape), but the selection is never written. Per this report, the fix
is to emit FFMUXA1.D6 (and FFMUXA2.D5 where FF2 takes the LUT O5)
and to stop routing same-slice LUT-O to FF-D through the fabric.

## Repro

- Census: `python3 tools/ffmux-corpus/census.py` in this worktree
  (writes `~/workspace/ffmux-corpus-logs/census-all14.json` and
  `census-all14.log`; exit 0, about 299 s).
- Segbits: `python3 tools/ffmux-corpus/segbits_check.py` (exit 0,
  384 FFMUX features, 0 bad).
- No build of production code was needed; the census reuses the
  lane worktree's `tools/us09v` as a library.
