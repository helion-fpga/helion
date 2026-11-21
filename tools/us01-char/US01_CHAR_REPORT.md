# US-01 Characterization Report (W-CHAR)

- Lane: `fm/ultra-bitgen-us01-mine`, characterization branch `fm/ultra-bitgen-us01-char`
- Date: Sat 2026-11-21
- Worker: W-CHAR, session be1334c8-c62a-4499-bc9b-6bb187de42a4
- Base commit: `6f8a8ba2cc3f0c12d20305b4176b62e2469dac56` (US-09 tip)
- Report path: `~/workspace/ultra-bitgen-crew/US01_CHAR_REPORT.md`
- Scripts: `tools/us01-char/` on the characterization branch
- Logs: `~/workspace/us01-char-logs/`
- Rules: license-free only, no Vivado, no vendor data beyond what is already
  public in the repo and corpus. No em dashes anywhere in this report.

## Headline verdicts

1. The US-01 premise is wrong for CLE slices. The BEL-pin-to-INT-wire map
   already exists in the public DBs: `tileconn.json` plus `tile_types` plus
   `site_types` give a complete, exact map. The prior check missed
   `tileconn.json` because it sits at depth 4 from `~/workspace` and the
   check stopped at depth 3. The router gap is that `helion-ultrascale`
   never loads `tileconn.json`, unlike `helion-xilinx7`, which does.
2. LUT inputs: DETERMINED. Every needed LUT input pin in P-11 (26,569) and
   P-13 (113,086) is either driven by a real decoded INT PIP or tied to
   VCC by the mux-default rule. Zero undetermined, zero decoder misses.
3. FF outputs: DETERMINED. Same construction, forward through INT PIPs.
4. CARRY8: DETERMINED, with one open modeling question (`DI{i}.DI{i}`
   source semantics, extract.py fails loud on it) and one unresolved flag
   (A-F7F8, needs a CTO ruling).
5. FF control pins (CLK/CE/SR): DETERMINED via tileconn CTRL wires plus
   RCLK PIPs.
6. DSP48E2: NOT MINEABLE from decoded FASM. Zero DSP lines decode in any
   pair and there is no DSP segbits file. The static map is complete
   (432/432 pins in tileconn), so the map itself needs no mining, but no
   decoded FASM can corroborate a DSP edge.
7. BRAM: PARTIAL. Config features decode (60,307 lines), but no BRAM site
   PIP ever appears in decoded or baseline FASM, so BRAM pin connections
   cannot be read from FASM. The static map is complete (RAMB18 203/203,
   RAMB36 533/533 pins in tileconn).
8. Ground truth: NO pair is fully verified today (corpus manifest section
   9). BNN-PYNQ P-13 to P-17 are the best ground-truth candidates
   (archived repo, in-repo .bit plus .hwh plus .tcl). Everything else can
   only corroborate until its source is pinned.

## Method note: the live-output rule was corrected twice

The first analysis used only FFMUX/OUTMUX selections to decide whether a
LUT output is live. That undercounted live LUTs twice, and both corrections
are recorded here because a critic will otherwise find the same holes:

- Correction 1, direct O6 output: the `{L}_O` site pin carries the LUT O6
  output straight to the INT fabric with NO OUTMUX/FFMUX selection. The
  checker's OUTMUX-missing error (extract.py lines 660-697) concerns only
  the `{L}MUX` pin. In P-11, 1,097 LUTs counted dead under the first rule
  are live through the direct O pin. Site pin `A_O` is an OUT pin in
  `site_type_SLICEL.json`; there is no separate O5 pin.
- Correction 2, carry paths: `CARRY8.DI{i}.DI{i}` feeds the LUT O6
  straight into the carry DI mux (183 LUTs in P-11, 657 in P-13 live only
  this way), and `S[i]` is the LUT O6 output, so when a slice's COUT is
  consumed (the slice above has a CIN site PIP) every S[i] matters
  through the ripple (9 slices in P-11, 4 in P-13 with no other
  carry-use selection).
- Correction 3, dual-5LUT mode: when both O5 and O6 are live the LUT is
  two 5-LUTs sharing A1-A5 and A6 is not an input. The naive 64-bit
  INIT-dependence test overcounted needed pins by 2,343 in P-11 before
  this rule was applied.

All numbers in this report use the final rule: O5 live if a D5 selection;
O6 live if a D6/XORIN/CY/F7F8 selection, or the `{L}_O` pin has
downstream INT PIP loads, or `DI{i}.DI{i}`, or the slice above has a CIN
site PIP. The decisive tie-off check was re-run after every correction.

## Q1: Decode reproduction

Command (per pair, after a release build):

```
cargo build --release -j1 -p helion-ultrascale --bin helion-bitgen
helion-bitgen decode --db-root ~/workspace/prjuray-db-ref/zynqusp \
  --part xczu3eg-sbva484-1-e --bit <file>
```

Build log: `~/workspace/us01-char-logs/build-release.log`. The build took
52.37s and produced `target/release/helion-bitgen` (5,693,752 bytes) with
only two pre-existing dead-code warnings (`NOOP_WORD` and `CMD_WCFG` never
used, `crates/helion-ultrascale/src/bitstream_packets.rs:255,270`).
Batch script: `tools/us01-char/decode-all.sh`. Per-pair stderr logs:
`~/workspace/us01-char-logs/decode-P-*.log`. Fresh decodes:
`~/workspace/us01-char-logs/decoded/P-*.fasm`. Summary:
`~/workspace/us01-char-logs/decode-summary.txt`.

All 14 `.bit` files were reused from disk at
`~/workspace/ultra-bitgen-crew/decoder-validation/bits/`; md5s verified
against the manifest, all 14 match exactly
(`~/workspace/us01-char-logs/bits-md5.txt`). No downloads were needed.

| Pair | FASM lines | Unexplained bits | Exit code |
| --- | --- | --- | --- |
| P-11 | 851,448 | 70,801 | 1 |
| P-13 | 1,630,780 | 227,110 | 1 |
| P-14 | 2,200,681 | 232,023 | 1 |
| P-15 | 1,955,110 | 231,932 | 1 |
| P-16 | 1,545,989 | 216,484 | 1 |
| P-17 | 2,244,884 | 227,309 | 1 |
| P-23 | 1,463,556 | 200,463 | 1 |
| P-28 | 1,048,060 | 123,632 | 1 |
| P-40 | 1,744,564 | 219,364 | 1 |
| P-41 | 1,809,151 | 215,489 | 1 |
| P-42 | 1,076,934 | 139,251 | 1 |
| P-43 | 1,631,778 | 210,067 | 1 |
| P-44 | 1,630,915 | 206,086 | 1 |
| P-45 | 1,256,335 | 202,812 | 1 |
| Total | 22,090,185 | 2,722,823 | |

Total unexplained bits is exactly 2,722,823, the expected number. Exit
code 1 on every pair is documented decoder behavior (it exits non-zero
when any bit is unexplained), not a decode failure. No pair failed to
decode.

### Counting trap, found and fixed

The first script run reported 2,722,837, off by 14. The count used
`grep -c '^unexplained:'`, which overcounts by 1 per pair because each
log ends with a summary line `unexplained: N set bits no feature
explains`, which is not a bit (documented in
`decoder-validation/UNEXPLAINED_ATTRIBUTION.md` line 38). The correct
count is `grep -c '^unexplained: FAR'`. `decode-all.sh` was corrected and
`decode-summary.txt` regenerated from the logs without re-decoding.

### Deviation: P-11 has 48 fewer lines than the stored decode

Fresh P-11 has 851,448 lines vs 851,496 in
`decoder-validation/decoded/P-11.fasm`. The 48 missing lines are all
`CMT_RIGHT_X32Y120.WIRE.CLK_HDISTR_0_*.USED.V0` and
`CLK_HROUTE_1_*.USED.V0`, all default V0 (off) values. The fresh file is
a strict subset: 0 lines only in the fresh file, 48 only in the old file.
The other 13 fresh files are byte-identical to the stored decodes (`cmp`
clean). No information is lost; the cause is not pinned down (possibly a
decoder-version difference in default WIRE emission). Recorded as a
deviation, not a failure.

### Runtime interruption, recorded

One background batch (the re-run of the analysis scripts after the
live-output corrections) was interrupted by a runtime restart at about
22:36 UTC. Its completed outputs were verified by file timestamp and
content (`analyze-P-11.json` finished at 22:35:33); the truncated and
stale outputs (`analyze-P-13.json` at 0 bytes, old `residual` and
`tieoff` files) were re-run to completion under `nohup setsid` and all
finished by 22:49. No numbers in this report come from the interrupted
run.

## Q2: Feature-class inventory

Classifier: `tools/us01-char/classify-fasm.py`. Per-pair JSON:
`~/workspace/us01-char-logs/classify-P-*.json`.

Classes: (a) BEL-pin-touching SLICE features split into a_lut (LUT INIT
lines), a_ff (FF INIT/SRVAL and related), a_carry8 (CARRY8 features),
a_slice_other (remaining SLICE features: CE/SR usage, mux selections,
LUTRAM mode, etc.); (b) b_int_pip (PIP lines on INT tiles); (c)
c_site_pip (PIP lines on SLICE/BRAM/DSP/IOB tiles); (d) d_bram, d_dsp,
d_clock, d_io, d_wire, d_pip_other, d_other.

Two classifier bugs were found and fixed during the work: PIP detection
used `".PIP." in rest` but FASM lines start `PIP.` (fixed to
`rest.startswith("PIP.")`); RAMB18/RAMB36 tiles were missed by the BRAM
check (fixed to match `RAMB18`/`RAMB36` prefixes).

| Pair | Total | a_lut | a_ff | a_carry8 | a_slice_other | b_int_pip | c_site_pip | d_bram | d_dsp | d_clock | d_io | d_pip_other | d_other |
| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |
| P-11 | 851,448 | 69,930 | 282,240 | 79,408 | 216,104 | 196,094 | 139 | 873 | 0 | 5,629 | 0 | 1,004 | 27 |
| P-13 | 1,630,780 | 67,248 | 282,240 | 79,664 | 257,650 | 921,670 | 976 | 6,647 | 0 | 13,671 | 170 | 813 | 31 |
| P-14 | 2,200,681 | 66,468 | 282,240 | 79,524 | 272,345 | 1,476,943 | 923 | 7,252 | 0 | 13,861 | 170 | 924 | 31 |
| P-15 | 1,955,110 | 64,610 | 282,240 | 79,535 | 268,930 | 1,235,826 | 889 | 8,151 | 0 | 13,843 | 170 | 885 | 31 |
| P-16 | 1,545,989 | 67,424 | 282,240 | 79,568 | 246,102 | 849,980 | 387 | 5,816 | 0 | 13,507 | 170 | 764 | 31 |
| P-17 | 2,244,884 | 66,619 | 282,240 | 79,408 | 256,912 | 1,537,838 | 307 | 6,311 | 0 | 13,979 | 170 | 1,069 | 31 |
| P-23 | 1,463,556 | 67,855 | 282,240 | 79,495 | 249,778 | 764,329 | 328 | 5,440 | 0 | 12,982 | 0 | 1,083 | 26 |
| P-28 | 1,048,060 | 68,834 | 282,240 | 79,424 | 226,070 | 380,690 | 78 | 1,669 | 0 | 8,221 | 0 | 795 | 39 |
| P-40 | 1,744,564 | 66,802 | 282,240 | 79,434 | 260,669 | 1,033,654 | 239 | 6,559 | 0 | 13,933 | 0 | 1,008 | 26 |
| P-41 | 1,809,151 | 66,209 | 282,240 | 79,434 | 263,243 | 1,096,590 | 239 | 6,515 | 0 | 13,666 | 0 | 989 | 26 |
| P-42 | 1,076,934 | 68,912 | 282,240 | 79,403 | 230,533 | 403,900 | 150 | 845 | 0 | 9,881 | 0 | 1,044 | 26 |
| P-43 | 1,631,778 | 68,806 | 282,240 | 79,501 | 263,500 | 919,610 | 166 | 1,516 | 0 | 14,277 | 0 | 2,136 | 26 |
| P-44 | 1,630,915 | 68,742 | 282,240 | 79,501 | 264,526 | 917,703 | 166 | 1,516 | 0 | 14,257 | 0 | 2,238 | 26 |
| P-45 | 1,256,335 | 67,627 | 282,240 | 79,399 | 232,841 | 579,232 | 167 | 1,197 | 0 | 12,288 | 0 | 1,318 | 26 |
| Total | 22,090,185 | 946,086 | 3,951,360 | 1,112,698 | 3,509,203 | 12,314,059 | 5,154 | 60,307 | 0 | 173,995 | 850 | 16,070 | 403 |

The totals sum to 22,090,185, matching the decode line count exactly.

### Line shapes, with real examples

- a_lut: `SLICE_X0Y0.A6LUT.INIT[63:0] = 0x8000000080000000` (the unused
  default, verified on the corner slice SLICE_X0Y0).
- a_ff: `SLICE_X0Y0.AFF.INIT.V1`, `SLICE_X0Y0.ABCDFF.CEUSED.V0`.
- a_carry8: `SLICE_X0Y0.CARRY8.CARRY_TYPE.SINGLE_CY8`.
- a_slice_other: includes FFMUX/OUTMUX selections, e.g.
  `SLICE_X0Y27.FFMUXA1.D6`, `SLICE_X0Y27.OUTMUXA.D5`.
- b_int_pip: `INT_X0Y10.PIP.BOUNCE_W_13_FT0.INT_NODE_IMUX_52_INT_OUT0`.
- c_site_pip: `SLICE_X10Y46.PIP.CLE_CLE_L_SITE_0_CIN_PIN.CLE_CLE_L_SITE_0_CIN`
  (CIN site PIPs are the only SLICE PIPs; see below).
- d_bram: `RAMB18_X0Y10.RAMB18E2_L.CLKARDCLKINV.V0`.
- d_clock: `CMT_RIGHT_X32Y0.WIRE.CLK_HDISTR_0_0.USED.V1`.
- d_io: `HDIO_BOT_RIGHT_X9Y120.HDIOLOGIC_M_X0Y0.OPFF.INIT.V0`
  (HDIO lines appear only in the BNN pairs P-13 to P-17, 170 each).
- d_pip_other: `RCLK_BRAM_INTF_TD_L_X12Y29.PIP.CLK_CMT_MUX_3TO1_10_CLK_OUT.CLK_TEST_BUF_SITE_5_CLK_OUT`,
  and `HDIO_BOT_RIGHT_X9Y120.PIP.HDIO_LOGICPAIR_14_OPFFM_Q.HDIO_LOGICPAIR_14_OPFFM_D1`.
- d_other: `INT_INTF_LEFT_TERM_PSS_X0Y179.OUTPUTS_ENABLED.1`.
- d_wire: 0 lines in every pair (no `.WIRE.` lines decode except the
  CMT ones counted under d_clock; the classifier puts CMT WIRE lines in
  d_clock).
- d_dsp: 0 lines in every pair. No DSP tile feature is decoded at all.

### Shapes of the data

- FASM is dominated by defaults. There are 8,820 SLICE sites in every
  pair. a_ff is exactly 282,240 in every pair (32 FF features per
  slice). The unused LUT INIT default is `0x8000000080000000`.
- Used LUTs (INIT not default): P-11 6,380 of 69,930 INIT lines; P-13
  33,488 of 67,248. The BNN pairs use far more LUTs than the resizer.
- P-11 SLICE templates: 257 distinct, enumerated by
  `tools/us01-char/templates-fasm.py`
  (`~/workspace/us01-char-logs/templates-P-11-SLICE.json`).
- Tile prefixes across all 14 pairs (top): INT 12,314,059; SLICE
  9,524,501; RCLK_INT_L 135,062; RAMB18 40,970; RCLK_INT_R 31,456;
  RAMB36 19,337; RCLK_HDIO 9,901; RCLK_DSP_INTF_CLKBUF_L 4,127;
  RCLK_BRAM_INTF_L 2,387; CMT_RIGHT 1,872; RCLK_BRAM_INTF_TD_L 1,456;
  RCLK_CLEM_L 996; RCLK_XIPHY_OUTER_RIGHT 936;
  RCLK_INTF_LEFT_TERM_ALTO 686; RCLK_DSP_INTF_L 476; HDIO_TOP_RIGHT
  470; HDIO_BOT_RIGHT 415; INT_INTF_LEFT_TERM_PSS 403;
  RCLK_BRAM_INTF_TD_R 336; RCLK_DSP_INTF_R 224; RCLK_CLEL_L_L 101;
  RCLK_CLEM_R 14. No DSP prefix and no URAM prefix anywhere.
- SLICE-only PIPs are CIN PIPs only. P-11 has 139 (95 CLEL + 44 CLEM).
  Total c_site_pip across pairs is 5,154, and every one checked is a
  CIN PIP.
- FFMUX/OUTMUX selections, P-11: FFMUX BYP 3,942, D6 2,230, D5 1,047,
  XORIN 795, F7F8 5, CY 5; OUTMUX D5 763, D6 691, XORIN 279, F7F8 34,
  CY 23. P-13: FFMUX BYP 18,930, D6 10,921, XORIN 7,913, D5 2,651,
  CY 106, F7F8 3; OUTMUX XORIN 4,453, D6 2,772, D5 1,841, F7F8 573,
  CY 42.
- CARRY8 DI defaults are DI0.AX through DI7.HX (about 8,756 each in
  P-11); non-default DI0.DI0 appears on 64 slices in P-11; CI.AX on 17
  slices, CI.V1 on 11 slices in P-11.
- LUTRAM/SRL features exist on SLICEM slices: WA7USED/WA8USED/WA9USED
  on 3,600 SLICEM slices in P-11; `{X}LUT.MODE.SRL/RAM`,
  `SIZE.SMALL/LARGE`, `DI1.DI1/SIN` features. See Q5 for the verdict.

## Q3: How a BEL pin connection is encoded

Tracer: `tools/us01-char/trace.py`. It loads tilegrid, tileconn, and the
decoded FASM PIPs, and follows tileconn stitches plus FASM PIPs. Bulk
drive analysis: `tools/us01-char/analyze-drive.py`. Residual analysis:
`tools/us01-char/residual.py`. Tie-off proof:
`tools/us01-char/check-tieoff.py`. Dead-LUT breakdown:
`tools/us01-char/dead-luts.py`. Carry-only liveness:
`tools/us01-char/carry-live.py`. No-live breakdown:
`tools/us01-char/nolive.py`.

### PIP direction convention

FASM `PIP.<dst>.<src>` means physical direction src to dst (from
`tools/us09v/us09v/fabric.py` line 7 and `routecheck.py`). So
`INT_X0Y24.PIP.IMUX_E10.INT_NODE_IMUX_14_INT_OUT0` means the INT_NODE
wire drives IMUX_E10. Tile names in FASM (INT_X0Y24) differ from grid
coords; grid coords in tilegrid are unique (66,385 distinct grids, 0
duplicates).

### Worked example 1: a used LUT input, P-11 SLICE_X0Y24 A1

Log: `~/workspace/us01-char-logs/trace-P-11-X0Y24-A1.log`. The slice is
in tile CLEL_R_X0Y24, type CLEL_R, grid (161,161), with non-default
A/B/C/D LUT INITs, ABCDFF CEUSED.V1, SRUSED.V1, SYNC.SYNC, and CFF/DFF
INIT.V0.

1. Site pin A1 maps by tileconn to the site wire
   `CLE_CLE_L_SITE_0_A1`, whose partner is (INT, IMUX_E10) at delta
   (-1,0), i.e. INT_X0Y24.
2. FASM line 4168: `INT_X0Y24.PIP.IMUX_E10.INT_NODE_IMUX_14_INT_OUT0`.
   The IMUX wire is driven by INT_NODE_IMUX_14_INT_OUT0.
3. FASM line 4237: `PIP.INT_NODE_IMUX_14_INT_OUT0.SS1_E_END4`. The
   INT_NODE wire is driven by SS1_E_END4.
4. Tileconn stitches SS1_E_END4 to SS1_E_BEG4 at INT_X0Y25.
5. FASM line 4571: `PIP.SS1_E_BEG4.INT_NODE_SDQ_20_INT_OUT1`.
6. FASM line 4520: `PIP.INT_NODE_SDQ_20_INT_OUT1.WW2_E_END4`.
7. Tileconn stitches WW2_E_END4 to (CLEL_R, EASTBUSOUT_FT1_12) at
   CLEL_R_X0Y25, a vertical bus wire inside a CLE tile.

At every hop there is exactly one driving PIP, so the chain is
hop-by-hop determined. It ends at a vertical bus wire whose driver
needs following the bus through CLE tiles, which was not completed in
this run. Recorded as determined but not fully terminated: the encoding
is a chain of INT PIPs plus tileconn stitches, and nothing in it is
ambiguous up to the bus wire.

### Worked example 2: an FF output, P-11 SLICE_X0Y27 AQ

Log: `~/workspace/us01-char-logs/trace-P-11-X0Y27-AQ.log`. The slice is
in tile CLEL_R_X0Y27, grid (161,158), with FFMUXA1.D6 and OUTMUXA.D5
(a dual 5-LUT use).

1. Site pin AQ maps by tileconn to `CLE_CLE_L_SITE_0_AQ`, partner
   (INT, LOGIC_OUTS_E14) at delta (-1,0), i.e. INT_X0Y27.
2. FASM line 4918: `PIP.INT_NODE_SDQ_17_INT_OUT0.LOGIC_OUTS_E14`.
3. FASM line 4987: `PIP.WW2_E_BEG3.INT_NODE_SDQ_17_INT_OUT0`.
4. Tileconn stitches WW2_E_BEG3 to INT_INTF_LEFT_TERM_PSS
   WESTBUSOUT_FT1_11, then EE2_W_END3.
5. FASM line 4884: `PIP.INT_NODE_IMUX_39_INT_OUT1.EE2_W_END3`,
   reaching another slice input region.

Forward tracing works the same way as backward tracing: the FF Q pin
maps to one INT wire by tileconn, and the FASM INT PIPs carry the net
forward hop by hop.

### Worked example 3: CARRY8, P-11 SLICE_X10Y45

Log: `~/workspace/us01-char-logs/trace-P-11-X10Y45-CIN.log`. The slice
is in tile CLEL_R_X6Y45, grid (187,139). CIN has 5 tileconn partners,
all delta (0,1) to the tile below CLEL_R_X6Y44: COUT of the slice
below, VCC_WIRE (termination), and COUT_FT0 variants. CIN does not
reach INT fabric at all.

- SLICE_X10Y45 is the chain head (CI.AX, DI0.DI0, no CIN PIP).
- SLICE_X10Y46, X10Y47, X10Y48 continue the chain, each with the CIN
  site PIP and default DI0.AX through DI7.HX.
- The chain is determined by CIN PIP presence plus tileconn COUT-to-CIN
  stitching plus the head CI feature (AX or V1).
- DI/CI are FASM mux selections, not wires: DI defaults AX through HX,
  non-default DI{n}; CI.AX or CI.V1. No INT wire is involved in the
  carry path except the X-pin bypass inputs.

The CIN-to-COUT stitch itself, verified directly against tileconn:
same-type CLE pairs (CLEL_L, CLEL_R, CLEM, CLEM_R) stitch
`CLE_CLE_L_SITE_0_CIN` to `CLE_CLE_L_SITE_0_COUT` of the slice below
at grid delta (0,1); at RBRK/TERM boundaries CIN stitches to COUT_FT0
(feedthrough) or VCC_WIRE (chain start/end termination); COUT stitches
to COUT_FT0/COUT_FT1 upward through feedthrough tiles. The CIN_PIN
wire has zero tileconn entries: it is tile-internal only, connected to
the CIN wire by the single real site PIP
`PIP.CLE_CLE_L_SITE_0_CIN_PIN.CLE_CLE_L_SITE_0_CIN` (the one
is_pseudo=0 PIP in tile_type_CLEL_L.json; the other 96 CLE PIPs are
pseudo-PIPs).

### The live-output rule, final form

For a used LUT (INIT not default), O5 is live if any FFMUX/OUTMUX D5
selection exists for its letter. O6 is live if any of these holds:

1. A D6/XORIN/CY/F7F8 FFMUX/OUTMUX selection for the letter.
2. The `{L}_O` site pin has downstream INT PIP loads (direct O6 output).
3. `CARRY8.DI{i}.DI{i}` for the letter (LUT O6 feeds the carry DI mux).
4. The slice above has a CIN site PIP (COUT consumed, S[i] is the LUT
   O6 output, so every S[i] matters through the ripple).

A needed input pin is then: A1-A5, if any live output's function
depends on it (INIT bit-flip test, A1 as LSB, confirmed against
us09v `extract.py` line 12: O6 = INIT[{A6..A1}], O5 =
INIT[{0,A5..A1}]); A6, only in true 6-LUT mode (O6 live without O5
live), because in dual-5LUT mode A6 is not an input.

Used-LUT output modes under the final rule:

| Pair | Used LUTs | O5 only | O6 only | Both | Neither (dead) |
| --- | --- | --- | --- | --- | --- |
| P-11 | 6,380 | 174 | 4,521 | 1,594 | 91 |
| P-13 | 33,488 | 274 | 28,823 | 3,191 | 1,200 |

The dead class (91 in P-11, 1,200 in P-13) is LUTs with non-default
INIT and no live output by any of the four paths. Breakdown
(`dead-luts.py`): all are plain MODE None, none are SRL or RAM mode,
and all have their carry DI from the X bypass pin, not from the LUT.
They are genuinely unconsumed (optimization residue). They need no
inputs routed, which the live rule handles.

### The VCC-default rule, and the decisive check

The US-09 decoder emits zero IMUX VCC_WIRE PIPs (P-11 decoded has 0;
the raw prjuray baseline has 89,333). The segbits DB encodes VCC PIPs
as all must-be-zero (`!`) bits, i.e. the mux default state, so a
set-bit decoder cannot see them. The only exception is
`INT.PIP.CTRL_W5.VCC_WIRE`, which has one set bit, so the decoder does
emit it (461 VCC_WIRE lines in P-11, all identical to baseline lines).
Documented in `decoder-validation/NORMALIZATION_REPORT.md`: 131,172
P-11 baseline-only INT lines end in `.VCC_WIRE`; 6,134,331 VCC lines
dropped by the normalizer across the corpus; rule R5. The normalizer
drops them because any set bit in that mux would surface as a real PIP
or an unexplained bit. There are no GND_WIRE IMUX PIPs in baseline
P-11 (count 0); ties are VCC (logic 1) only. Example:
`INT_X6Y45.PIP.IMUX_E18.VCC_WIRE` in the baseline is the tie-off for
SLICE_X10Y45 A6, which the decoded FASM leaves undriven.

Decisive check (`tools/us01-char/check-tieoff.py`, outputs
`tieoff-P-11.json`, `tieoff-P-13.json`): for every needed LUT input
pin, compare its IMUX wire against both the decoded FASM and the raw
prjuray baseline FASM.

| Pair | Needed pins | Real-driven (decoded) | VCC-tied (baseline) | Baseline-real-only | Truly undriven | No INT partner | Problems |
| --- | --- | --- | --- | --- | --- | --- | --- |
| P-11 | 26,569 | 25,869 | 700 | 0 | 0 | 0 | none |
| P-13 | 113,086 | 111,262 | 1,824 | 0 | 0 | 0 | none |

Every needed pin is either driven by a real decoded INT PIP or tied to
VCC in the baseline. Zero decoder misses, zero undetermined pins. The
undriven-but-needed residual (`residual-P-11.json` 700 pins, by pin
A6 394 and A1 306; `residual-P-13.json` 1,824 pins, A6 1,483 and A1
341) is fully explained by the VCC rule. The residual examples are
mostly carry slices (FFMUX XORIN, DI fed by LUT), e.g. SLICE_X10Y45
A6, INIT 0xafa0a0afcccccccc, FFMUXA1 XORIN, carry DI0.DI0, FF Q loads
3, LUT O loads 0: the A6 input is VCC-tied, which is the correct
tie-off for that carry slice, not a missing net.

### FF control pins

Tileconn partners for control pins (same for all CLE tile types):
CLK1 to INT CTRL_E4 (delta -1,0) and CTRL_W4 (delta 1,0); CLK2 to
CTRL_E5/W5; CKEN1 to CTRL_E0/W0; SRST1 to CTRL_E6/W6; LCLK to CTRL_W8
only; WCKEN to CTRL_W9 only. The RCLK PIPs
(`CLK_LEAF_SITES_*_CLK_IN`, `CLK_LEAF_SITES_*_CE_INT`) also name site
clock pins. FF D input is determined site-internally by the FFMUX
selection (BYP/D5/D6/XORIN/CY/F7F8); no wire mapping is needed for D.

FF slices with any FFMUX entry: P-11 1,189, P-13 5,640. FF Q loads
(AQ forward drive): P-11 0 loads 715, 1 load 319, 2 loads 135, 3 loads
20; P-13 0 loads 3,204, 1 load 1,770, 2 loads 554, 3 loads 112.

## Q4: prjuray-db inventory for xczu3eg

### tileconn.json exists: the prior check looked one level too shallow

`~/workspace/prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tileconn.json`
exists: 84,343,553 bytes, 81,640 entries, 658,732 wire pairs, 57,961
distinct wire names, 1,445 distinct tile-type pairs. A second copy
exists for xczu3eg-sfvc784-1-e. 7-series copies exist under
`~/workspace/xilinx-dbsrc/artix7/xc7a50t/tileconn.json` and
`~/workspace/xilinx-dbsrc/zynq7/xc7z020/tileconn.json`.

Verification of the prior negative: `find ~/workspace -maxdepth 3
-name 'tileconn.json'` returns nothing, reproducing the prior check.
`find ~/workspace -maxdepth 4 -name 'tileconn.json'` returns all four
files. The file sits at depth 4 from `~/workspace`
(prjuray-db-ref/zynqusp/xczu3eg-sbva484-1-e/tileconn.json), one level
deeper than the prior check looked. That is the whole reason the US-01
premise said no public source maps BEL pins to INT wires.

Entry shape: `{grid_deltas: [dx, dy], tile_types: [A, B], wire_pairs:
[[wireA, wireB], ...]}`. It stitches wires between adjacent tiles,
including BEL-pin wires to INT wires. The repo already loads tileconn
for 7-series: `crates/helion-xilinx7/src/db/load.rs` (load_tileconn)
and `device.rs` build fabric from tileconn stitching with union-find.
The ultrascale harness doc (`crates/helion-ultrascale/harness/
SOURCES.md` line 18) calls tileconn.json files "81M each, not needed
by the harness", which is how the file was overlooked.

### tilegrid.json

Dict of 66,385 tiles. Re-confirmed: 0 tiles have a `wires` key;
14,329 sites total, 0 with `site_pins`. Sites are a dict mapping site
name to a string type, e.g. a CLE tile:
`{"sites": {"SLICE_X7Y0": "SLICEL"}, "type": "CLEL_L"}`.
`pin_functions` is non-empty on only 16 tiles (SYSMONE4, IOB entries).
CLE tile counts: CLEL_R 4,500, CLEM 2,520, CLEM_R 1,080, CLEL_L 720,
total 8,820, matching the 8,820 decoded SLICE sites. Other tile types
present include NULL 46,778, INT 5,940, INT_INTF_L 1,080, INT_INTF_R
900, INT_INTF_L_PCIE4 360, INT_INTF_R_PCIE4 360.

### site_types: 41 files

`~/workspace/prjuray-db-ref/zynqusp/site_types/site_type_*.json`.
`site_type_SLICEL.json` has `site_pins` with IN/OUT directions
matching the tileconn pins (A1-A6 IN, AQ/AQ2/AMUX OUT, AX IN, etc.).
Also present: `site_type_SLICEM.json`, `site_type_DSP48E2.json` (432
pins), `site_type_RAMB181.json` (203 pins), `site_type_RAMBFIFO36.json`
(533 pins), `site_type_RAMBFIFO18.json`, plus BUFG/MMCM/IO/HDIO/
BITSCLICE types. The us09v checkers already consume these
(`fabric.py` line 264 reads `site_pins` direction).

### tile_types: 158 files

`~/workspace/prjuray-db-ref/zynqusp/tile_types/tile_type_*.json`.
`tile_type_INT.json`: pips 3,774 (keys like
`INT.BOUNCE_E_BLN_13_FT1->>INT_NODE_IMUX_22_INT_OUT0`), wires 1,475,
sites 0. `tile_type_CLEL_L.json`: 97 PIPs, 323 wires, 1 site. The 97
CLE PIPs are 96 pseudo-PIPs (is_pseudo 1: each of A1-A6 through H1-H6
to `{letter}MUX` and `{letter}_O`) plus 1 real PIP (CIN to CIN_PIN,
is_pseudo 0). Same 97-pip structure in CLEM and CLEL_R (CLEL_R wires
319 vs 323). CLEM_R, RBRK and TERM variants exist as files
(tile_type_CLEL_L_RBRK, CLEL_L_TERM_B/T, CLEL_R_TERM_B/T, CLEM_RBRK,
etc.). `tile_type_DSP.json`: 196 PIPs (all is_pseudo 0, site-pin to
`_PIN` wires, e.g. `DSP.DSP_DSP48_0_ACIN_B0->DSP_DSP48_0_ACIN_B0_PIN`),
2,074 wires, 2 sites. `tile_type_BRAM.json`: 893 PIPs (all is_pseudo
0), 2,701 wires, 3 sites. `tile_type_CMT_RIGHT.json`: 4,083 PIPs, 63
sites, 7,316 wires.

### segbits

Under `~/workspace/prjuray-db-ref/zynqusp/`: segbits_bram,
segbits_bram.block_ram, segbits_clel_l, segbits_clel_r, segbits_clem,
segbits_clem_r, segbits_cmt_right, segbits_hdio_bot_right,
segbits_hdio_top_right, segbits_hpio_right, segbits_int,
segbits_int_intf_left_term_pss, segbits_rclk_ams_cfgio,
segbits_rclk_bram_intf_l, segbits_rclk_bram_intf_td_l,
segbits_rclk_bram_intf_td_r, segbits_rclk_clel_l_l, segbits_rclk_clem_l,
segbits_rclk_clem_r, segbits_rclk_dsp_intf_clkbuf_l,
segbits_rclk_dsp_intf_l, segbits_rclk_dsp_intf_r, segbits_rclk_hdio,
segbits_rclk_int_l, segbits_rclk_int_r,
segbits_rclk_intf_left_term_alto, segbits_rclk_xiphy_outer_right,
segbits_xiphy_byte_right, each with a `.origin_info.db`. There is NO
DSP segbits file and NO URAM segbits file, matching the corpus
manifest section 6 note. segbits_bram.db is a TEXT file (feature plus
bit lists), not SQLite, and contains 0 lines with `PIP`:
`BRAM.RAMB18E2_L.CLKARDCLKINV.V0 !05_91` is a typical line. So BRAM
site PIPs have no segbits encoding at all. segbits_int.db has 3,778
PIP lines.

### Other files in xczu3eg-sbva484-1-e

`part.json` (35,564 bytes), `part.yaml`, `package_pins.csv` (14,501
bytes), `tilegrid.json` (15,000,392 bytes). `timings/` holds SDF files
(RAMB181.sdf, RAMBFIFO36.sdf, SLICEL.sdf, SLICEM.sdf, etc.).

### Tileconn coverage per site type (the numbers the miner needs)

| Site type | Site pins | Pins with a tileconn stitch | Missing |
| --- | --- | --- | --- |
| SLICEL | 106 | 106 | 0 |
| SLICEM | 108 | 108 | 0 |
| DSP48E2 | 432 | 432 | 0 |
| RAMB181 | 203 | 203 | 0 |
| RAMBFIFO36 | 533 | 533 | 0 |

Coverage is exact for every site type that matters. SLICEL has 106
pins and SLICEM 108 in tileconn, matching site_types exactly; the two
extra SLICEM pins are LCLK and WCKEN (LUTRAM clock and write enable).
CLE A1 appears 4 times in tileconn, once per CLE tile type (CLEL_L,
CLEL_R, CLEM, CLEM_R), each mapping to IMUX_W10 or IMUX_E10 as
appropriate; the 4th occurrence (CLEM_R) is `(CLEM_R, INT)` delta
(1,0) to IMUX_W10. DSP wires are named `DSP_DSP48_<0|1>_<pin>` (2
sites per tile); BRAM wires `BRAM_BRAM_CORE_<1|2|3>_<pin>` (3 sites
per tile). No SLICE SITE_1 wires exist: the only SITE_1/3/5/7 wires in
tileconn are CLK_TEST_BUF CLK_IN pins, and XIPHY_BITSLICEWRITE sites
14/15. Each CLE tile has exactly one SLICE site, SITE_0.

### What is absent

- URAM: no site_type file, no tile_type file, no tilegrid type, no
  segbits file, and no URAM tile in any decoded pair. xczu3eg has no
  URAM blocks, so no URAM coverage is needed for this part.
- DSP segbits: absent, so no DSP config feature can decode.
- `~/workspace/xilinx-dbsrc` holds 7-series data only (artix7, zynq7,
  prjxray-full, tools). No UltraScale content: a find for ultra/xczu/
  xcku under it returns nothing. Out of scope for this lane except as
  a note.

## Q5: Mineability verdicts per BEL-pin class

### LUT inputs: DETERMINED

The site pin to INT wire edge is fixed by tileconn (one partner per
pin per tile type; 106/106 SLICEL pins, 108/108 SLICEM pins). The
driving net is fixed by the FASM INT PIPs plus the VCC-default rule.
Proof: the tie-off check covers every needed pin in P-11 (26,569) and
P-13 (113,086) with zero problems. The encoding cannot confuse pins:
each LUT input has its own IMUX wire (A1 to IMUX_W10/E10, A2 to
IMUX_W8/E8, A3 to IMUX_W24/E24, A4 to IMUX_W22/E22, A5 to
IMUX_W26/E26, A6 to IMUX_W18/E18 for CLEL_L/CLEM, E variants for
CLEL_R), and each IMUX wire has at most one driving PIP in the
decoded FASM. What remains ambiguous: nothing for the pin-to-wire
edge. What the FASM alone cannot tell you is which IMUX wires are
VCC-tied, because the decoder drops VCC PIPs; the miner must apply
the VCC-default rule (an IMUX wire with no driving PIP in the decoded
FASM and a VCC PIP in the baseline is tied to 1), not treat it as
undriven. The unexplained bits cannot hide a missing INT PIP: INT-
attributed unexplained bits are tiny (48 in P-11, 115-454 in the other
pairs, per UNEXPLAINED_ATTRIBUTION.md), and the H-1 band, which
accounts for most of the gap, is now explained as a computed code
carrying no routing information.

### FF outputs (Q, Q2): DETERMINED

Same construction as LUT inputs in reverse: the Q pin maps to one
LOGIC_OUTS wire by tileconn (AQ to LOGIC_OUTS_W14/E14), and the FASM
INT PIPs carry the net forward. Q loads were counted (P-11: 715 zero,
319 one, 135 two, 20 three, of 1,189 FF slices; P-13: 3,204 zero,
1,770 one, 554 two, 112 three, of 5,640 FF slices). Zero-load Qs are
FFs whose output goes nowhere (or only into the same slice); they need
no routing.

### FF D input: DETERMINED site-internally

The D input is the FFMUX selection (BYP/D5/D6/XORIN/CY/F7F8). BYP
takes D from the X pin (a site input, mapped by tileconn like any
other pin); D5/D6 take the LUT outputs; XORIN/CY take the carry
outputs. No wire mapping beyond the pin map is needed. Counts are in
Q2 (P-11 FFMUX BYP 3,942, D6 2,230, D5 1,047, XORIN 795, F7F8 5,
CY 5).

### FF control pins (CLK/CE/SR): DETERMINED

Tileconn maps CLK1/CLK2/CE/SR pins to INT CTRL wires (see Q3), and the
RCLK PIPs (`CLK_LEAF_SITES_*_CLK_IN`, `CLK_LEAF_SITES_*_CE_INT`)
name the site clock pins. The clock network itself (which CMT/BUFG
drives which CTRL wire) is visible in the d_clock and d_pip_other
features (173,995 and 16,070 lines). No control pin was found without
a tileconn partner.

### CARRY8 pins: DETERMINED, with two open items

DI/CI are FASM mux selections, CIN/COUT is tileconn stitching plus
CIN PIP presence (see Q3). The chain head is identified by the CI
feature (AX/V1) plus absence of a CIN PIP; continuation slices by
their CIN PIP. Open item 1: `DI{i}.DI{i}` source semantics. The
checker (extract.py lines 24-26, 765, 810-815) treats `DI{i}.DI{i}`
as unmodelled and fails loud, and S[i] is defined as the LUT O6
output. This report counts a LUT as live when its DI is `DI{i}`,
which is the only reading consistent with the checker's own
`_carry_used` predicate (any `DI{i}.DI{i}` makes the carry used), but
the exact Verilog the extractor should emit for it is not settled.
64 slices use DI0.DI0 in P-11; 183 LUTs in P-11 and 657 in P-13 are
live only through a DI feed. Open item 2: the F7/F8/F9 mapping is
assumption A-F7F8 (flag from US-09, unresolved, needs a CTO ruling).
FFMUX/OUTMUX F7F8 counts are small (P-11: 5 FFMUX, 34 OUTMUX; P-13:
3 FFMUX, 573 OUTMUX), so the exposure is bounded but the mapping is
not validated against a Vivado F7/F8 build yet.

### DSP48E2 pins: NOT MINEABLE from decoded FASM

Zero DSP lines decode in any pair, and there is no DSP segbits file,
so no DSP pin connection can be read from the decoded output. The
static DB side is complete (432/432 site pins in tileconn; tile_type
_DSP.json has 196 PIPs, all real, site-pin to `_PIN` wires), so the
map itself needs no mining: the miner should take the DSP pin-to-wire
edges from tileconn exactly as for CLE. What is missing is
corroboration: a DSP block's use (which pins matter in a given
design) cannot be read from decoded FASM, because the DSP config bits
are unexplained. DSP-attributed unexplained bits are part of the
2,722,823 total. A miner that wants to validate DSP edges from
bitstreams would first need a DSP segbits file, which does not exist
in the public DBs.

### BRAM pins: PARTIAL

RAMB18/RAMB36 config features decode (60,307 lines total; P-11 873,
P-13 6,647). P-13 has 126 RAMB18 tiles, each with L and U halves and
about 51 config features, all shown values being defaults (e.g.
`RAMB18E2_L.CLKARDCLKINV.V0`, `ENADDRENA.FALSE`,
`RSTREG_PRIORITY_A.RSTREG`); 51 distinct RAMB18 templates. But the
features are config (clock inversion, enable inversion, reset
priority, clock domains), not pin connections: no BRAM site PIP ever
appears in decoded FASM (0 RAMB PIP lines in P-13 decoded) or in the
raw baseline FASM either (0 RAMB PIP lines in P-13 baseline). The
tile_type_BRAM.json PIPs (893, all is_pseudo 0) have no segbits
encoding (segbits_bram.db has 0 PIP lines), so they never surface in
FASM. The BRAM pin-to-wire edges exist in tileconn (RAMB18 203/203,
RAMB36 533/533 pins) and should be taken from there, as for DSP.
BRAM-attributed unexplained bits are the largest attributed class in
P-11 (478) and in P-13 to P-17 (about 3,000-3,400 each), per
UNEXPLAINED_ATTRIBUTION.md; those bits are BRAM content/config bits
the decoder does not model, not pin connections.

### LUTRAM / SRL (SLICEM): features exist, extraction blocked

SLICEM features decode (WA7USED/WA8USED/WA9USED on 3,600 SLICEM
slices in P-11; `{X}LUT.MODE.SRL/RAM`, `SIZE.SMALL/LARGE`,
`DI1.DI1/SIN`). The LUTRAM write-data and address pins (WCKEN, LCLK,
and the A1-A6 inputs in RAM mode) map through tileconn like any other
pin, so the pin-to-wire edges are determined. But the memory behavior
itself (SRL shift, RAM read/write) is an extraction error in the
checker (extract.py: "LUTRAM and SRL modes are extraction errors
(blocked, no public encoding)"). The miner can map the pins; it cannot
validate the memory semantics from FASM alone. Note the dead-LUT
breakdown found zero SRL/RAM-mode LUTs among the dead class, so the
SRL/RAM LUTs in these pairs do have live outputs by the four-path
rule.

### IO / HDIO: not deeply traced

850 d_io lines total, all in the BNN pairs P-13 to P-17 (170 each).
Example HDIO PIP: `HDIO_BOT_RIGHT_X9Y120.PIP.HDIO_LOGICPAIR_14_OPFFM_Q.HDIO_LOGICPAIR_14_OPFFM_D1`.
HDIO site types exist in site_types and tile_types, and the IO pin
map is presumably in tileconn the same way, but no IO pin was traced
end to end in this lane. Verdict: likely determined by the same
construction, not verified here. The PS-side pins (PSS_ALTO, 409
attributed unexplained bits in P-11) are out of scope: the PS is not
routable fabric.

### Clock: determined at the RCLK level

173,995 d_clock lines and 16,070 d_pip_other lines (mostly RCLK PIPs).
The RCLK PIPs name site clock pins directly
(`CLK_CMT_MUX_3TO1_10_CLK_OUT.CLK_TEST_BUF_SITE_5_CLK_OUT`). The
clock network is a separate routing problem from the INT fabric, but
its encoding in FASM is explicit, not implied.

## Q6: Ground-truth assessment

Source: `~/workspace/ultra-bitgen-crew/public-bitstream-corpus.md`
(manifest sections 1, 6, 9). The manifest states plainly: all 44 pairs
are header-verified only, and NO pair is FULLY VERIFIED. Source
pin-down is open for every pair. Nothing in this section claims a pair
is ground truth; it ranks which pairs could become ground truth and
what pinning each needs.

Speed-grade split, relevant to all pairs: P-11, P-23, P-40 to P-45 are
xczu3eg-sbva484-1-e; P-13 to P-17 and P-28 are xczu3eg-sbva484-1-i.
The prjuray DB ships tilegrids only for -1-e and -sfvc784-1-e, so the
-1-i pairs were decoded against the -1-e DB (documented in
VALIDATION_REPORT.md). For mining, the pin map is assumed identical
between -1-e and -1-i (same die, different speed grade); that
assumption is stated, not verified.

| Pair | Design | Source status | Ground-truth role |
| --- | --- | --- | --- |
| P-11 | Resizer, PYNQ-HelloWorld, Ultra96 | Known repo, unpinned. Boards/ultra96/resizer TCL plus HLS C++, hosted .bit, matching-era tag pynq-helloworld 2.7 era, not verified. Repo master is now PYNQ v3.0 era. | Corroborate only until pinned to the 2.7-era tag. |
| P-13 | BNN-PYNQ CNV, Ultra96 | Archived repo, in-repo .bit plus .hwh plus .tcl under bnn/bitstreams/ultra96/, make-hw.sh, Vivado 2018.2. | Best candidate. Archived tree is frozen, so the .bit is pinned by the tree; reproduction still unproven. |
| P-14 | BNN-PYNQ CNV, Ultra96 | Same as P-13. | Best candidate. |
| P-15 | BNN-PYNQ CNV, Ultra96 | Same as P-13. | Best candidate. |
| P-16 | BNN-PYNQ LFC, Ultra96 | Same as P-13. | Best candidate. |
| P-17 | BNN-PYNQ CNV, Ultra96 | Same as P-13. | Best candidate. |
| P-23 | FINN, Ultra96 | Release v0.0.7a zip, .bit plus .hwh in zip. RTL is FINN-generated (HLS plus IP Integrator block design). | Corroborate only. Grounding needs the pinned FINN flow (FINN v0.10.1 plus Vivado 2022.2), not just checked-in RTL. |
| P-28 | PYNQ-ComputerVision xv2Filter2DDilate, Ultra96 | In-repo .bit plus .hwh plus .so. Rebuild path SDSoC 2018.2/2018.3 (old flow). | Corroborate only; old flow. Second tier after BNN. |
| P-40 | FINN, Ultra96 | Same as P-23. | Corroborate only. |
| P-41 | FINN, Ultra96 | Same as P-23. | Corroborate only. |
| P-42 | FINN, Ultra96 | Same as P-23. | Corroborate only. |
| P-43 | FINN, Ultra96 | Same as P-23. | Corroborate only. |
| P-44 | FINN, Ultra96 | Same as P-23. | Corroborate only. |
| P-45 | FINN, Ultra96 | Same as P-23. | Corroborate only. |

Practical consequence for the miner: the map does not need ground
truth to be built, because it already exists in tileconn. Ground truth
is needed to VALIDATE the map against real designs, and the validation
that matters most is the tie-off style check already done here (every
needed pin driven or VCC-tied, zero problems, on two pairs from
different design families). A held-out validation should use a BNN
pair (P-13 to P-17) against a FINN pair (P-23, P-40 to P-45), since
those are the two largest independent families. P-11 (resizer) is the
smallest and least representative.

## Deviations and open items

1. P-11 fresh decode has 48 fewer lines than the stored decode, all
   default V0 CMT WIRE lines. No information loss; cause unpinned.
2. First unexplained-bit count was 2,722,837, off by 14, due to the
   summary-line trap. Corrected count is exactly 2,722,823.
3. The live-output rule was corrected twice (direct O pin, carry
   DI/COUT paths). The tie-off check was re-run after each correction;
   final numbers are in Q3. Any earlier draft quoting 437 or 520
   residual pins for P-11, or 1,087 or 1,167 for P-13, is superseded:
   final residuals are 700 (P-11) and 1,824 (P-13), all VCC-tied.
4. The LUT-input trace (SLICE_X0Y24 A1) terminates at a vertical bus
   wire (EASTBUSOUT_FT1_12) in a CLE tile; the bus continuation was
   not followed. The chain is determined hop by hop up to that wire.
5. `DI{i}.DI{i}` source semantics are an open modeling question
   (extract.py fails loud). The miner must treat a DI-fed LUT as live,
   but the exact Verilog emission is not settled.
6. Flag A-F7F8 (F7/F8/F9 mapping) is unresolved and needs a CTO ruling.
7. IO/HDIO pins were not traced end to end. Likely determined by the
   same construction, not verified.
8. The -1-e vs -1-i pin-map identity assumption is stated, not
   verified.

## Recommendations for the miner

1. Do not mine the CLE pin-to-wire map from bitstreams. Load
   `tileconn.json` (plus `tile_types` and `site_types`) in
   `helion-ultrascale`, the way `helion-xilinx7` already does, and
   treat the bitstreams as validation data.
2. Validation to run first: the tie-off check (`check-tieoff.py`)
   held out on a FINN pair (e.g. P-40) and on P-28, after building
   the map from tileconn alone. Expect zero problems if the map is
   right.
3. Apply the VCC-default rule in the miner: an IMUX wire with no
   driving PIP in decoded FASM is VCC-tied if the baseline has a VCC
   PIP for it, and undriven otherwise. Never emit a VCC PIP as a
   routed net.
4. Use the final live-output rule (four paths) when deciding which
   LUT inputs a design needs. The naive mux-only rule undercounts
   live LUTs by over a thousand per pair.
5. For DSP and BRAM, take the pin-to-wire edges from tileconn and do
   not expect FASM corroboration: no DSP segbits exist, and BRAM
   site PIPs have no segbits encoding and never appear in FASM.
6. Do not use any pair as ground truth until its source is pinned per
   Q6. The BNN pairs are the first pinning target.

## Appendix: commands and files

Decode, build, and batch:

```
cargo build --release -j1 -p helion-ultrascale --bin helion-bitgen
helion-bitgen decode --db-root ~/workspace/prjuray-db-ref/zynqusp \
  --part xczu3eg-sbva484-1-e --bit <file>
grep -c '^unexplained: FAR' decode-P-*.log
```

Analysis (all under `tools/us01-char/` on branch
`fm/ultra-bitgen-us01-char`):

```
python3 classify-fasm.py <fasm>            # per-pair class counts
python3 templates-fasm.py <fasm> <prefix>  # template enumeration
python3 trace.py <fasm> <site> <pin>       # BEL-pin trace
python3 analyze-drive.py <fasm>            # bulk drive analysis
python3 residual.py <fasm>                 # undriven-needed breakdown
python3 check-tieoff.py <decoded> <baseline>  # VCC-default proof
python3 nolive.py <fasm>                   # no-mux-entry LUT breakdown
python3 carry-live.py <fasm>               # DI-fed and COUT-only classes
python3 dead-luts.py <fasm>                # dead used-LUT breakdown
```

Key logs (`~/workspace/us01-char-logs/`): `build-release.log`,
`decode-all.log`, `decode-P-*.log` (14), `decode-summary.txt`,
`decoded/P-*.fasm` (14 fresh decodes), `bits-md5.txt`,
`classify-P-*.json` (14), `templates-P-11-SLICE.json`,
`templates-P-13-RAMB18.json`, `analyze-P-11.json`,
`analyze-P-13.json`, `residual-P-11.json`, `residual-P-13.json`,
`tieoff-P-11.json`, `tieoff-P-13.json`,
`trace-P-11-X0Y24-A1.log`, `trace-P-11-X0Y27-AQ.log`,
`trace-P-11-X10Y45-CIN.log`, `nolive-P-11.json`,
`nolive-P-13.json`, `carrylive-P-11.json`, `carrylive-P-13.json`,
`dead-P-11.json`, `dead-P-13.json`.

References: `~/workspace/ultra-bitgen-crew/US09_REPORT.md`,
`~/workspace/ultra-bitgen-crew/public-bitstream-corpus.md`,
`~/workspace/ultra-bitgen-crew/decoder-validation/` (baseline/,
decoded/, normalized/, VALIDATION_REPORT.md, NORMALIZATION_REPORT.md,
UNEXPLAINED_ATTRIBUTION.md, fasm_normalize.py),
`~/workspace/prjuray-db-ref/zynqusp/` (tilegrid, tileconn, tile_types,
site_types, segbits_*.db, timings),
`~/workspace/helion-ultra-bitgen-us01-char/tools/us09v/us09v/`
(fabric.py, routecheck.py, extract.py, fasmio.py, db.py),
`~/workspace/helion-ultra-bitgen-us01-char/crates/helion-ultrascale/src/fabric.rs`,
`bitstream_decode.rs`, `segbits.rs`.
