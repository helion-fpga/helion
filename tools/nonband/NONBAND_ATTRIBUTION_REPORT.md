# Non-band unexplained-bit attribution report

Lane branch: `fm/ultra-bitgen-nonband`, base `9c4917de` (the n76n77n78fix tip).
Worktree: `~/workspace/helion-ultra-bitgen-nonband`, HEAD `9c4917de`, clean.
Report: `~/workspace/ultra-bitgen-crew/NONBAND_ATTRIBUTION_REPORT.md`
(copy committed byte-identical at `tools/nonband/NONBAND_ATTRIBUTION_REPORT.md`
on the lane branch).
Date: Nov 26, 2026. Worker: agent b67236e9-5240-45b2-9c24-7d102d3916ef.
Orchestrator session 20 closes the lane; no merges into fm/ultra-bitstream,
fm/zedboard-merge, or master. No critic dispatched by this lane; critic
dispatch stays with the main agent.

No em dashes anywhere in this file.

## 1. Outcome

Zero segbits lines derived. This is a proven-blocked outcome with evidence,
not a failure to look: the full per-tile-type recurrence table is built
(section 3), every windowed position was tested for an exact FASM correlate,
and no position meets the derivation criterion with a unique, implementable
line. One slice is fully explained by lines that already exist (INT,
section 5); every other slice is proven unpinnable from this corpus with a
named unblocker (sections 4, 6, 7, 8). No emitter change was made, so no
full acceptance run applies; the targeted suites pass (section 9).

Per-type remainder table (windowed attributed subset, 30,503 bits):

| tile type | bits | distinct (frame, bit) | derived this lane | remainder | verdict |
|---|---|---|---|---|---|
| BRAM | 27367 | 64 | 0 | 27367 | unpinnable, no exact correlate |
| INT | 2634 | 8 | 0 | 2634 | explained by existing DB lines, blocked on decoder shared-bit handling |
| CMT_RIGHT | 232 | 32 | 0 | 232 | unpinnable, ambiguous attribution plus a falsifying exception |
| INT_INTF_R_PCIE4 | 120 | 3 | 0 (already derived Nov 22) | 0 | closed, OUTPUTS_ENABLED |
| HDIO_TOP_RIGHT | 90 | 18 | 0 | 90 | unpinnable, degenerate plus elimination-only evidence |
| HDIO_BOT_RIGHT | 5 | 1 | 0 | 5 | unpinnable, same as HDIO_TOP_RIGHT |
| RCLK_CLEM_L | 20 | 2 | 0 | 20 | unpinnable, ambiguous or no correlate |
| RCLK_BRAM_INTF_L | 15 | 2 | 0 | 15 | unpinnable, no exact correlate |
| RCLK_INT_L | 14 | 10 | 0 | 14 | unpinnable, single-design or no exact correlate |
| RCLK_BRAM_INTF_TD_L | 2 | 1 | 0 | 2 | unpinnable, ambiguous (3 exact correlates) |
| RCLK_CLEL_L_L | 2 | 1 | 0 | 2 | unpinnable, ambiguous (4 exact correlates) |
| RCLK_DSP_INTF_L | 2 | 2 | 0 | 2 | unpinnable, single design |
| gap columns (out of scope) | 24404 | n/a | 0 | 24404 | excluded, Q6 Vivado probe unblocker |
| TOTAL | 54907 | 144 windowed positions | 0 | 30503 windowed | |

The derivation criterion applied throughout: a (frame, bit) is derivable
only if it appears exactly when a decoded FASM feature is present,
cross-validated on at least two designs, and the resulting line is unique
(one feature, not several matching identically) and implementable (no
duplicate of an existing vendored line, which the loader rejects).
Elimination-only evidence (every known claimant fails, so the setter must
be an unlisted feature) does not meet this bar, because it cannot name the
feature the bit belongs to. Near matches are recorded, not rounded up.

## 2. Totals re-verified independently

The lane did not take the probe totals on trust. A fresh attribution was
computed from the decode logs alone
(`~/workspace/n7-port-logs/validate/corpus-decode/P-*.log`, strict regex
`^unexplained: FAR 0x[0-9a-fA-F]+ bit [0-9]+$`), mapping each FAR to a
tile window from `~/workspace/intintf-logs/all-windows.log` with block,
row, column, minor, and absolute bit kept. Script:
`~/workspace/nonband-logs/attribute.py`, output
`~/workspace/nonband-logs/attribute.out`,
recurrence JSON `~/workspace/nonband-logs/recurrence.json`.

- Total unexplained: 54,907. Gap: 24,404 in columns 57 (7682), 42 (6537),
  88 (4313), 11 (3562), 82 (1944), 101 (356), 30 (10). Windowed: 30,503.
  All three numbers match the N7_PORT_REPORT table and the fresh
  INT_INTF probe logs exactly, per pair and in total.
- Per-pair windowed: P-11 550, P-13 3265, P-14 3560, P-15 3306, P-16 3586,
  P-17 3674, P-23 2498, P-28 922, P-40 2658, P-41 2620, P-42 549,
  P-43 1232, P-44 1330, P-45 753.
- Distinct windowed (type, block, frame, bit) positions: 144. All of them
  are block 0 (CLB_IO_CLK). Zero unexplained bits fall in any BLOCK_RAM
  block, and the decoded FASM contains zero RAMB INIT lines, so no corpus
  design sets a BRAM INIT content bit.
- The probe log format drops the FAR block, so the block-aware
  re-attribution above is the authority for the BRAM block split, not the
  probe logs. The two agree on every per-type total.

## 3. The recurrence table

For every windowed position, the table records the designs it appears in,
the distinct tiles, and the instance count. Full table:
`~/workspace/nonband-logs/recurrence.json` (144 entries). Headlines:

- BRAM dominates: 27,367 of 30,503 windowed bits (89.7 percent), 64
  positions, 1,115 used tile-instances across the 14 designs. Every used
  BRAM tile carries unexplained bits (14 to 28 per tile); every tile
  carrying unexplained BRAM bits is a used tile.
- INT: 2,634 bits, 8 positions, 2,634 tile-instances (one bit per
  instance, never two on one tile).
- CMT_RIGHT: 232 bits, 32 positions, 3 tiles only
  (CMT_RIGHT_X32Y0, X32Y60, X32Y120).
- HDIO: 95 bits on 2 tiles only (HDIO_TOP_RIGHT_X9Y150 with 18 positions,
  HDIO_BOT_RIGHT_X9Y120 with 1), all in the 5 BNN designs P-13 to P-17.
- RCLK types: 57 bits total, 6 types, 18 positions, mostly 1 to 2 designs.
- INT_INTF_R_PCIE4: 120 bits, 3 positions, already derived (section 8).

Candidate FASM features for each position were taken from the decoded
FASM of the same designs (`~/workspace/n7-port-logs/validate/corpus-decode/`),
per tile instance, so a bit and its candidate are compared on the same
tiles in the same designs.

## 4. BRAM: 27,367 bits, unpinnable

### 4.1 Structure

The 64 positions collapse into 33 distinct instance-sets: 26 groups of
2 to 3 positions that always co-occur, plus 7 singletons. The groups:

- Five frame 1-3 triples, all-or-nothing per tile (0 partial triples in
  1,115 tiles): (1,21),(2,27),(3,24) n=942; (1,69),(2,75),(3,72) n=704;
  (1,117),(2,123),(3,120) n=773; (1,165),(2,171),(3,168) n=644;
  (1,213),(2,219),(3,216) n=595. Triple presence per tile: all five on
  588 tiles, only triple 0 on 173, none on 166, triples 0-2 on 95, and
  six rarer patterns. The patterns vary within designs (for example
  P-13 has 63 all-five tiles, 44 only-triple-0 tiles, and 4 none tiles),
  so they are tile-level properties, not design-level ones.
- Six frame 4-5 pairs with matching counts: (4,42),(5,34) n=88;
  (4,43),(5,37) n=183; (4,44),(5,40) n=691; (4,45),(5,31) n=9;
  (4,46),(5,28) n=17; (4,47),(5,25) n=563.
- Frame 4 pairs: (4,87),(4,167) n=164; (4,94),(4,174) n=3;
  (4,95),(4,175) n=157; (4,127),(4,136) n=286; (4,196),(4,214) n=19;
  (4,197),(4,218) n=11; (4,202),(4,219) n=592; (4,204),(4,220) n=184;
  (4,205),(4,221) n=94; (4,134),(4,135) n=8; (4,81),(4,161) n=7;
  (4,86),(4,166) n=5; (4,80),(4,160) n=2.
- Frame 5 groups: (5,82),(5,85) n=1110; (5,103),(5,125) n=1115 (every
  used tile); (5,169),(5,172) n=1014.
- Singletons: (4,84) n=1094; (4,149) n=1003; (4,194) n=477; (4,211) n=474;
  (4,47) n=563; (5,25) n=558; (5,128) n=706.

Zero of the 64 positions is claimed by any line in the public
`segbits_bram.db` (270 lines, frames 04 and 05 only; no frame 01 to 03
line exists at all). The frames 1 to 3 triples sit in a region the public
DB never mentions.

### 4.2 The correlation search, and why it fails

Three searches, all over the same universe of 1,115 used
(pair, tile) instances, script `~/workspace/nonband-logs/bram_corr.py`
and `~/workspace/nonband-logs/bram_bitmask.py`, outputs
`bram_corr.out`, `bram_bitmask.out`, `bram_bitmask.json`:

- Single decoded FASM feature (1,108 distinct features): no exact match
  for any of the 64 positions, except the degenerate always-on ones:
  (5,103) and (5,125) are present on all 1,115 tiles, and so are about
  20 always-on features (for example RAMB18E2_L.SLEEP_ASYNC.FALSE,
  RAMB18E2_L.CLKBWRCLKINV.V0, RAMB36E2.EN_ECC_WRITE.FALSE). An always-on
  feature matches every always-on bit identically, so it pins nothing.
- Property values including absence (for example WRITE_MODE_A absent,
  meaning the WRITE_FIRST default, which emits no line): no exact match.
  Two-property combinations (L plus U halves) and tile coordinates
  (X, Y mod 5, Y mod 10): no exact match.
- Bitmask search over 1,187 properties: single exact matches only for
  the degenerate (5,103)/(5,125) cases; pair conjunctions: none; pair
  AND-NOT: none.

Two tiles in P-13 illustrate the dead end directly. BRAM_X12Y110
(all five triples) and BRAM_X12Y100 (only triple 0) have identical
decoded feature sets except DOA_REG (V1 on the first, V0 on the second),
and BRAM_X21Y85 (no triples) matches the second except for its
unexplained bits. No decoded property separates the triple patterns.

### 4.3 Verdict and unblocker

BRAM is proven unpinnable from the decoded FASM: the driver of every
position is invisible in the decoded features. Leading hypothesis, stated
as a hypothesis and not a derivation: the bits encode per-port data
widths and modes beyond the public DB's coverage (the DB carries only
READ_WIDTH_A.36, WRITE_WIDTH_B.36, and the three WRITE_MODE values, so
every other width has no segbits line and no FASM line), and possibly
CASCADE or FIFO configuration, for which no segbits file exists at all.
The 5-triple plus 6-pair grouping is consistent with per-port width and
mode fields, but the corpus cannot assign a width value to any tile, so
no line is forced. Unblocker: a Vivado fuzz job with known per-port
widths and modes (M2), or the FINN design sources giving each BRAM its
configured width and mode, correlated against these same 33
instance-sets. The instance-sets in `recurrence.json` are the exact
target: any future width assignment must reproduce them.

## 5. INT: 2,634 bits, mechanism proven, no new line

All 8 unexplained INT positions are the unique positive select bits of
the CTRL_W5 mux values in the public `segbits_int.db`:

| position | value | instances |
|---|---|---|
| (37,33) | INT_NODE_GLOBAL_14_INT_OUT0 | 1063 |
| (47,33) | INT_NODE_GLOBAL_0_INT_OUT1 | 851 |
| (38,32) | INT_NODE_GLOBAL_5_INT_OUT1 | 260 |
| (36,32) | INT_NODE_GLOBAL_8_INT_OUT0 | 210 |
| (37,32) | INT_NODE_GLOBAL_6_INT_OUT0 | 146 |
| (36,33) | INT_NODE_GLOBAL_1_INT_OUT0 | 63 |
| (39,32) | INT_NODE_GLOBAL_3_INT_OUT1 | 31 |
| (46,31) | INT_NODE_GLOBAL_10_INT_OUT1 | 10 |

Each value's DB line lists 9 bits: its own positive select bit plus the
other 8 negated, including `!21_35`. Bit (21,35) is shared: it is also
the positive bit of `INT.PIP.INT_NODE_SDQ_80_INT_OUT1.LOGIC_OUTS_W22`.
Script `~/workspace/nonband-logs/int_check.py` and `int_check2.py`,
outputs `int_check.out`, `int_check2.out`. The result is exact in both
directions on all 14 designs:

- Every one of the 2,634 unexplained INT bits sits on a tile with NO
  decoded CTRL_W5 line and WITH the SDQ LOGIC_OUTS_W22 PIP line.
- Every tile WITH a decoded CTRL_W5 line has zero unexplained INT bits
  (0 in all 14 designs).
- Tiles with both an SDQ line and a decoded CTRL_W5 line number 40
  across the corpus, and all 40 select VCC_WIRE, the one value whose
  positive bit is 21_35 itself.
- Tiles with SDQ and no CTRL_W5 line number 2,634, and every one carries
  exactly one unexplained INT bit (the select bit of its value).
- No unexplained INT bit exists on any tile without the SDQ line.

So the decoder emits no CTRL_W5 line exactly when the SDQ PIP sets
21_35, because that breaks every GLOBAL value's `!21_35` requirement,
and the value's unique select bit is left unexplained. The segbits lines
already exist in `segbits_int.db`; nothing is missing from the DB. What
is missing is decoder handling of the shared-bit conflict: emitting the
value identified by its unique positive select bit when the only failed
requirement is the shared 21_35 set by the SDQ PIP. That is a decoder
change, outside this lane's segbits-derivation scope, and it would change
decoded FASM output across the corpus, so it is flagged here as a
follow-up for the orchestrator, not implemented. No new segbits line is
derived for INT.

## 6. CMT_RIGHT: 232 bits, unpinnable

Three tiles only (X32Y0, X32Y60, X32Y120), 32 positions, 0 of them
claimed by any line in `segbits_cmt_right.db`. Script
`~/workspace/nonband-logs/small_corr.py`, output `small_corr.out`.
Exact single-feature correlates exist for five HDISTR wire groups:

| wire (V1 line present) | unexplained bits tracking it | instances | designs |
|---|---|---|---|
| CLK_HDISTR_0_8 | (5,22),(5,78),(5,87),(5,93) | 11 | 5 |
| CLK_HDISTR_0_2 | (7,22),(7,28),(7,37),(7,43) | 16 | 6 |
| CLK_HDISTR_0_20 | (7,21),(7,27),(7,36),(7,42) and (10,22),(10,78),(10,87),(10,93) | 6 | 2 |
| CLK_HDISTR_0_14 | (8,22) and (9,55),(9,64),(9,70) | 8 | 3 |
| CLK_HDISTR_0_3 | (6,22),(6,28),(6,37),(6,43) | 4 | 2 |

The DB's V1 line for each wire lists exactly one bit (for example
0_8 lists only 05_84), which is set and explained on the same
instances. The tracked bits are additional bits set in exactly the same
instances. Two readings are consistent with the exact correlation:

- (a) The true V1 encoding is 5 bits and the public line is incomplete.
- (b) The extra bits belong to a co-active feature, most plausibly the
  driver mux select for the wire's source, which is the same in every
  design that uses the wire here.

Reading (a) is contradicted as a general rule by the (6,21) group:
those 4 bits track HDISTR_0_1 usage except for one instance,
(P-11, CMT_RIGHT_X32Y0), where 0_1 is used but the bits are absent.
A wire-encoding bit cannot be absent when the wire is used. The bits
therefore track something narrower than wire usage, and the corpus
cannot separate the wire from its driver: no design uses any of these
wires with a different driver. The (7,23) group (n=2) appears in one
design only and fails the two-design bar. No line is derived. The
implementability problem is separate and also blocks reading (a): the
V1 lines already exist in the vendored DB, the loader fails loud on a
duplicate feature name in a new file, and the precedent forbids editing
a vendored DB in place, so an extended V1 line has no home without a
loader or DB-policy decision. Unblocker: a Vivado design that uses one
of these HDISTR wires with a different driver, or the M2 CMT fuzz,
which can vary wire and driver independently.

## 7. HDIO: 95 bits, unpinnable

All 95 bits sit on one TOP tile (HDIO_TOP_RIGHT_X9Y150, 18 positions,
90 bits) and one BOT tile (HDIO_BOT_RIGHT_X9Y120, 1 position, 5 bits),
constant across the 5 BNN designs P-13 to P-17, which are identical in
HDIO usage (same 7 OPFF PIP lines, same 7 IOSTANDARD_OUT lines, verified
by identical md5). The other 9 designs use no HDIO tiles and carry no
HDIO bits. Any constant HDIO feature therefore correlates, so design-level
correlation is degenerate.

### 7.1 The six OSERDES bits: elimination-complete, still not derived

Five TOP bits and the one BOT bit are exactly the negated bits of the
OSERDES_T_BYPASS.TRUE rows in the public HDIO DBs, one per logic site:

- TOP: (1,221) HDIOLOGIC_S_X0Y0, (1,413) S_X0Y1, (1,893) S_X0Y3,
  (1,1085) HDIOLOGIC_M_X0Y4, (1,1373) S_X0Y5.
- BOT: (1,29) HDIOLOGIC_M_X0Y0.

Within the TOP tile the 12 OSERDES TRUE bits split 5 set (the 5 above)
and 7 clear (M_Y0, M_Y1, M_Y2, M_Y3, M_Y5, S_Y2, S_Y4: not unexplained
and not claimed by any emitted line, hence clear). The TRUE feature is
never emitted anywhere in the corpus (0 OSERDES_T_BYPASS lines in P-13
and the same in the other designs), because the decoder only emits a
negated-only feature when a complement feature exists in the DB, and
none does for these bits (see `decode_frames` in
`bitstream_decode.rs`, the `observable` gate). The DB has no FALSE row.

For (1,221) the elimination was checked in full: its only positive
claimers in the DB are three IOB_X0Y1 IOSTANDARD lines, and all three
fail on other bits (IN_OUT.LVCMOS18 and IN_OUT combined require
!01_193, but 01_193 is set, explained by the emitted
OUT.LVCMOS18_IDRIVE_I12_SLEW_SLEW_SLOW line; OUT.LVCMOS33_I8_SLOW
requires !01_194, but 01_194 is set). The positive claimers of the
other four TOP bits are the same shape (IN_OUT and OUT lines of
neighboring IOB sites whose emitted values differ). So no known feature
can set these bits, and the only remaining DB claimant is the TRUE
row's negated bit. That makes OSERDES_T_BYPASS.FALSE the leading
reading, but it is not forced: the bit could equally be set by an
unfuzzed feature that coincides with this position (for example a
TFFM-path bit), and the corpus has no design where the same site's
bypass is TRUE, so the FALSE reading has no positive cross-check.
The per-site split also does not match any emitted feature: OQ_MUX.
NOT_OPTFF is emitted on M_Y4, S_Y0, S_Y1, S_Y3, S_Y4 (4 of the 5
set sites, plus S_Y4 whose bit is clear; S_Y5 has the bit set and no
OQ_MUX line at all). No derivation. Unblocker: a Vivado design with
OSERDES_T_BYPASS TRUE on a site (the bit must then be clear), or the
M2 HDIO fuzz.

### 7.2 The thirteen IOB_X0Y3 bits

The remaining TOP bits, (0,387),(0,413),(0,421),(0,422),(0,424),
(0,429),(1,385),(1,386),(1,387),(1,417),(1,421),(1,423),(1,429), are
all claimed by IOB_X0Y3 IOSTANDARD value lines. X0Y3 has only
IOSTANDARD_IN.LVCMOS18 and PULLTYPE.NONE emitted: it is an input-only
pin. The bits are consistent with the OUT-absent encoding of that pin,
for which the DB has no line, and no corpus design gives X0Y3 an
output, so there is no variation to pin them against. No derivation.
Same unblocker as 7.1.

## 8. RCLK types: 57 bits, unpinnable

Script `~/workspace/nonband-logs/small_corr.py`, output
`small_corr.out`. The pattern in every multi-design case is the same:
either no exact correlate, or several exact correlates, so no unique
line is forced.

- RCLK_CLEM_L, (9,35) n=12, 6 designs, 2 tiles: THREE exact correlates
  (WIRE.CLK_VROUTE_BOT.USED.V1, PIP.CLK_VROUTE_BOT.
  CLK_CMT_MUX_3TO1_2_CLK_OUT, WIRE.CLK_VDISTR_BOT.USED.V1). The bit is
  claimed by no DB line. Three features match identically, so the bit
  cannot be uniquely attributed. (9,42) n=8, 2 designs, 7 tiles: no
  correlate at all.
- RCLK_BRAM_INTF_L, (3,35) n=14, 7 designs, 2 tiles: only near matches
  (one false negative each). (0,42) n=1, 1 design: fails the bar.
- RCLK_INT_L, 14 bits, 10 positions: 9 positions are single-design;
  (19,7) is 2 designs but has no exact correlate (best is one false
  positive). No line.
- RCLK_BRAM_INTF_TD_L, (0,42) n=2, 2 designs, 2 tiles: THREE exact
  correlates (two WIRE lines and one PIP line). Ambiguous, no line.
- RCLK_CLEL_L_L, (9,42) n=2, 2 designs, 2 tiles: FOUR exact correlates.
  Ambiguous, no line.
- RCLK_DSP_INTF_L, (4,42) and (5,42) n=1 each, 1 design: fails the bar.

The recurring bit 42 across RCLK types (frames 0, 4, 5, 9) correlates
with CLK_TEST_BUF_SITE lines in several cases, which points at the
clock test-buffer enable as the likely family, but the exact feature
is not identified by this corpus. Unblocker: a Vivado design varying
the test-buffer and mux usage independently, or the M2 RCLK fuzz.

## 9. INT_INTF and gap bits: scope notes

INT_INTF_R_PCIE4 (120 bits) is closed by the Nov 22 lane and was not
re-derived here: all 120 are OUTPUTS_ENABLED.1 at (1,21),(2,27),(3,24)
on 8 tiles in the 5 BNN designs, already wired into the emitter. The
decoder stays public-DB-only by design, so the decode output still
reports them unexplained; that is intended, not a remainder.

Gap bits (24,404, columns 57, 42, 88, 11, 82, 101, 30) are out of scope
for this lane per the dispatch brief: they are blocked on the missing
xczu3eg tilegrid (Q6 Vivado probe, named unblocker). No gap-bit work
was attempted.

## 10. Acceptance scope

No derivation succeeded, so the full acceptance list does not apply;
only the targeted suites were run, on the untouched base (no code
changed, worktree clean apart from this report):

- `cargo test -j2 -p helion-ultrascale --lib`: 262 passed, 0 failed,
  0 ignored. Log: `~/workspace/nonband-logs/test-lib.log`. This matches
  the INT_INTF lane's fresh run at the same base.
- `cargo test -j2 -p helion-ultrascale --test device_file_xczu3eg`:
  9 passed, 0 failed. Log: `~/workspace/nonband-logs/test-devicefile.log`.
  This includes `xczu3eg_intintf_pip_still_fails_unknown_feature`, which
  still expects UnknownFeature for the INT_INTF PIP: B1 stays blocked.

The 12 flows, 12 verify-bits, and blinky .bit md5 were not re-run:
nothing in the emitter, decoder, or data changed, so no flow number can
have moved.

## 11. What remains

- BRAM 27,367 bits: needs Vivado width and mode fuzz (M2) or FINN
  sources; the 33 instance-sets in `recurrence.json` are the target.
- INT 2,634 bits: needs a decoder fix for the CTRL_W5 versus SDQ
  shared-bit 21_35 conflict (section 5). Recommended follow-up, not
  implemented by this lane.
- CMT_RIGHT 232 bits: needs a Vivado design varying HDISTR wire usage
  against its driver, or the M2 CMT fuzz.
- HDIO 95 bits: needs a Vivado design with OSERDES_T_BYPASS TRUE on a
  site and with an output on IOB_X0Y3, or the M2 HDIO fuzz. The six
  OSERDES bits are the strongest near-miss in the lane.
- RCLK 57 bits: needs a Vivado design varying test-buffer and mux usage
  independently, or the M2 RCLK fuzz.
- Gap 24,404 bits: Q6 Vivado probe for the xczu3eg tilegrid.
- INT_INTF PIP segbits (B1): unchanged, still blocked on M2 or Q6.

## 12. Evidence index

- Logs and scripts: `~/workspace/nonband-logs/` (`attribute.py`,
  `attribute.out`, `recurrence.json`, `bram_corr.py`,
  `bram_corr.out`, `bram_corr.json`, `bram_cluster.py`,
  `bram_cluster.out`, `bram_unexp.json`, `bram_bitmask.py`,
  `bram_bitmask.out`, `bram_bitmask.json`, `int_check.py`,
  `int_check.out`, `int_check2.py`, `int_check2.out`,
  `small_corr.py`, `small_corr.out`, `test-lib.log`,
  `test-devicefile.log`).
- Corpus probe logs: `~/workspace/intintf-logs/` (14 `P-*.probe.log`,
  `all-windows.log`).
- Decoded FASM and decode logs:
  `~/workspace/n7-port-logs/validate/corpus-decode/`.
- Public DBs: `~/workspace/prjuray-db-ref/zynqusp/` (`segbits_*.db`,
  `tile_types/`).
- Prior lane reports: `~/workspace/ultra-bitgen-crew/INTINTF_REPORT.md`,
  `~/workspace/ultra-bitgen-crew/INTINTF_SEGBITS_REPORT.md`.
- Worktree: `~/workspace/helion-ultra-bitgen-nonband` (branch
  fm/ultra-bitgen-nonband, HEAD 9c4917de; only new file is the report
  copy at `tools/nonband/NONBAND_ATTRIBUTION_REPORT.md`, plus build
  artifacts under the worktree `target/` directory).
