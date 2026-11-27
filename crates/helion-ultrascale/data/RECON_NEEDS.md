# RECON_NEEDS: every Vivado fact the UltraScale+ flow still needs

Date: 2026-11-26. Vivado 2017.3 is up on the USF cluster right now.
DeepSeek is running the UltraScale recon; Claude Code's device dumps (M11)
are done. This file lists every Vivado fact the Helion UltraScale+ flow
still needs for synthesis, placement, routing and bitgen, per part and per
board, and marks each one COVERED (a running recon job produces it),
PARTIAL (a running job covers it only in part or only for one part) or
MISSED (no running job produces it; run it before cluster time ends).

Update Nov 26, 2026 (evening): DeepSeek U1 licensing probe results landed
(see Licensing reality below). M11 is done. M1, M2, M5-M8 and M10 are
submitted on xcku3p; U3, U4 and U6 are being resubmitted. xczu3eg is
still being probed.

Scope rule from the goal: one-time Vivado use for fuzzing DB generation is
allowed; the shipped tool stays license-free. Nothing in this file asks
Vivado to do the flow for us. Every item is a fact (a table, an encoding,
a delay, a bitstream) that Helion then consumes from its own data files.
No merges; this file lives on its own fm/ branch.

Parts: xcku3p (primary, xcku3p-ffva676-1-e), xcvu3p (xcvu3p-ffvc1517-1-e),
xczu3eg (proving part, end to end flows today), xczu9eg (ZCU102),
xcku040 (new), xcku5p (licensed bit-source, no device file in the repo
yet). Boards: ZCU102 is xczu9eg. Other eval boards are TBD in this file;
the golden-bitstream job is per board part, so the part is the unit that
matters.

## Licensing reality (DeepSeek U1 probe, Nov 26, 2026)

U1 probed synthesis licensing per part on the cluster. This is the
authority for what the recon can and cannot produce. It supersedes the
Oct 22 finding (synth_design for xcvu3p failed with no Synthesis license)
by extending it to every part.

- Licensed: xc7z020, the small 7-series parts, xcku3p, xcku5p.
- NOT licensed: xcvu3p, xcvu9p, xcvu095, xcku040, xczu9eg, xc7k325t,
  xc7vx485t, xc7z045, xc7s25.
- xczu3eg is being probed; no result yet.

Consequences, stated plainly:

- No Vivado goldens, timing, or fuzz bitstreams can come from xcvu3p,
  xczu9eg, or xcku040. Everything that needs bits comes from xcku3p and
  xcku5p (and from xczu3eg if its probe comes back licensed).
- No-license work runs on every part: link_design -part dumps of tiles,
  sites, pins, and properties need no synthesis license. This is how M11
  was completed for the unlicensed parts.
- Tier 2 (zero diff vs a real Vivado bitstream) is achievable only on
  xcku3p and xcku5p (and xczu3eg if licensed). For xcvu3p, xczu9eg, and
  xcku040, Tier 2 can only be approached through carried-over tile-type
  encodings from xcku3p plus the M11 link_design dumps (see section 5);
  a same-part golden is impossible.
- The "license: Webpack" field in xcku3p.json and xcvu3p.json is the
  public license tier, not the cluster license state. Do not confuse the
  two; U1 is the authority for cluster work.
- DeepSeek is resubmitting U3, U4, and U6, and submitting M1, M2, M5-M8,
  and M10 on xcku3p now. U3/U4/U6 scopes were not restated in the report;
  presumably they are re-scoped to the licensed parts.

## 0. What we already have (public sources, no Vivado needed)

Do not re-collect these. The recon should not spend cluster time on them.

- Segbits DBs from prjuray-db (CC0, zynqusp family, i.e. xczu3eg class):
  INT 3778 lines, HPIO right 1560, HDIO top right 812, HDIO bot right 828,
  CMT right 104, CLEL_L/R 768 each, CLEM/CLEM_R 829 each. Provenance in
  `PROVENANCE.md`. These cover the Zynq UltraScale+ family only. They do
  NOT cover xcku3p or xcvu3p.
- Site-type JSONs (pins only, same source): SLICEL, SLICEM, DSP48E2,
  BUFGCE, BUFGCTRL, RAMB181, RAMBFIFO36, HPIOB_M/S, HDIOLOGIC_M/S, MMCM,
  PLL. No URAM288 site type exists anywhere public.
- LUT/FF/BRAM bit locations from BitFiltrator ULTRASCALE_PLUS.json (MIT):
  `ultrascale_plus_lut_ff_bram.json` (LutLoc, RegLoc, BramMemLoc 16384,
  BramMemParityLoc 2048).
- Device files: `xcku3p.json` (27,120 slices, 162,720 LUTs, 325,440 regs,
  1,368 DSPs, 360 BRAMs, 1 SLR) and `xcvu3p.json` (provisional: 49,260
  slices, 394,080 LUTs, 788,160 regs, 2,280 DSPs, 720 BRAMs).
- INT PIP audit: `tile_type_INT.json` enumerates 3774 tile PIPs; 3770
  directional match the INT segbits DB, 4 bidirectional as FWD/REV pairs.
- BEL-pin-to-INT-wire map: public tileconn.json (81,640 entries, 658,732
  wire pairs), loaded at runtime via HELION_TILECONN_DB_ROOT. US-01 is
  closed as a routing-graph model for xczu3eg; DSP/BRAM pins stitch to
  INT_INTF tiles.
- Harness: 60 records, 14 green, 46 blocked, 0 red. Tier 1 green: LUT_INIT,
  FF_INIT, INT_PIP, FRAMING, BRAM_CONTENT 0/16384, BRAM_PARITY 0/2048.
- Documented xczu3eg timing: exactly one value, BRAM CLK to DOUT 1020 ps
  (DS925 v1.30 Table 80 TRCKO_DO, -2/0.85V). Everything else is
  Ballpark/UNKNOWN.

## 1. Needs by flow stage

### A. Device model

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Tilegrid: tile types, locations, FAR layout per part | xcku3p, xcvu3p, xczu9eg, xcku040, xczu3eg | M11 device dumps (every package of every UltraScale and UltraScale+ family); Claude Code tiles dump (xcku3p, xczu9eg, xcku040); DeepSeek tile templates | COVERED: M11 dumps cover every package of every family, including xcvu3p and xczu3eg. Dumps live on the cluster side; they still need to land in the repo as data files. |
| Site models: site types, pins, site PIPs per part | xcku3p, xcvu3p, xczu9eg, xcku040, xczu3eg | M11 device dumps; Claude Code site models (xcku3p, xczu9eg, xcku040) | COVERED via M11, same landing note as tilegrid. |
| Package pins: pin, bank, I/O standard, signal per part | xcku3p, xcvu3p, xczu9eg, xcku040, xczu3eg | M11 device dumps; Claude Code package pins (xcku3p, xczu9eg, xcku040) | COVERED via M11, same landing note as tilegrid. |
| xcvu3p frame total (US-06, GAP-F9): UG570 says 71820, our file constructs 73035, overshoot 1215 | xcvu3p | DeepSeek DEVICE_MODEL_GAPS xcvu3p | COVERED |
| xcvu3p slice count: AMD XMP103 says 49,250, our file says 49,260 | xcvu3p | DeepSeek DEVICE_MODEL_GAPS xcvu3p | COVERED |
| xcku3p slice count: AMD says 20,340, our file says 27,120; frame sum 41460 vs UG570 41476 (16 frames) | xcku3p | DeepSeek DEVICE_MODEL_GAPS is xcvu3p only; Claude Code tiles for xcku3p give geometry | PARTIAL: the dispute needs a Vivado tilegrid read on xcku3p, which the tiles dump provides; no named job checks the counts against it |
| xcvu3p FAR rows, clock-region rows/cols, bram/dsp col majors: all constructed in our file, none Vivado-confirmed | xcvu3p | DeepSeek DEVICE_MODEL_GAPS xcvu3p | COVERED |
| Per-part IDCODE and frame layout (FAR rows, frame counts) for xcku040 and xczu9eg | xcku040, xczu9eg | M11 device dumps (link_design, no license needed) | COVERED via M11. Goldens cannot supply this: both parts are unlicensed. |
| Part access: license checkout works for each part (probe, not a claim) | all five | DeepSeek part access probe (U1) | DONE (U1, Nov 26): licensed on xc7z020, small 7-series parts, xcku3p, xcku5p. NOT licensed on xcvu3p, xcvu9p, xcvu095, xcku040, xczu9eg, xc7k325t, xc7vx485t, xc7z045, xc7s25. xczu3eg probe pending. |

### B. Synthesis mapping

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Primitive properties: INIT values, IS_*_INVERTED defaults, property ranges per primitive | all five | Claude Code primitive properties (xcku3p, xczu9eg, xcku040); M11 primitive properties (xcvu3p, xczu3eg) | COVERED: M11 adds primitive properties for xcvu3p and xczu3eg. Same landing note as section A. |
| DSP48E2 inference ground truth: which multiply/add patterns Vivado maps to DSP48E2, with which attributes | xcku3p | M10 submitted on xcku3p (Nov 26) | IN FLIGHT on xcku3p; mapping rules carry by primitive, not by part (see section 5) |
| CARRY8 inference ground truth: which arithmetic patterns become chained CARRY8 | xcku3p | M10 submitted on xcku3p (Nov 26) | IN FLIGHT on xcku3p; carries by primitive (see section 5) |
| LUTRAM/SRL inference ground truth: which patterns become LUTRAM vs SRL16E/SRL32E vs LUTs | xcku3p | M10 submitted on xcku3p (Nov 26) | IN FLIGHT on xcku3p; carries by primitive (see section 5) |

### C. Pack and place

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Tile geometry for legality: CLB column layout, DSP/BRAM/URAM column positions, clock-region boundaries | all five | M11 device dumps; Claude Code tiles + site models; DeepSeek tile templates | COVERED via M11 for all five parts (geometry is link_design data, no license needed) |
| Control-set limits (max unique control sets per slice, per UG574) | family | none named; doc knowledge, not Vivado data | MISSED (low priority: read UG574, no cluster time needed) |
| CARRY8 chain placement: vertically contiguous slice columns that accept chains | xcku3p | tile templates give geometry | COVERED if the templates include CLB column adjacency |
| IO/DSP/BRAM column legality vs tilegrid | xcku3p, xcvu3p | Claude Code tiles (xcku3p) + DEVICE_MODEL_GAPS (xcvu3p) + M11 dumps | COVERED via M11 dumps |

### D. Routing

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| INT PIP segbits completeness per part (public INT DB is zynqusp only) | xcku3p, xcvu3p | DeepSeek xcku3p fuzz corpus (single-feature/pair sweeps) | PARTIAL: xcku3p covered; xcvu3p has no same-part corpus (M12 blocked, unlicensed) and must use carried-over xcku3p encodings (see section 5) |
| INT_INTF tile wiring map (BEL-pin-to-INT-wire for DSP/BRAM/URAM pins) | xcku3p, xcvu3p | DeepSeek tileconn; Claude Code tiles + site models; M11 dumps | PARTIAL: xczu3eg map is public tileconn.json; xcku3p/xcvu3p maps come from the recon tileconn dump and M11 dumps |
| INT_INTF_R_PCIE4 PIP segbits (B1, the IOB placement lane blocker; emitter fails loud UnknownFeature, zero public files contain IMUXOUT16) | xcku3p (and any part whose IOB routes through INT_INTF_R_PCIE4) | M2 submitted on xcku3p (Nov 26) | IN FLIGHT on xcku3p; segbits carry to any part with the same INT_INTF_R_PCIE4 tile type (confirm existence in the M11 dumps) |
| IO_CONFIG 55 bits (GAP-B2, Q6 Vivado PinPlanning probe) | xcku3p | M1 submitted on xcku3p (Nov 26, Q6 PinPlanning probe) | IN FLIGHT on xcku3p; bit meanings carry by HPIO/HDIO tile type (see section 5) |
| HDIO/HPIO IOB segbits per part (public HDIO/HPIO DBs are zynqusp only) | xcku3p, xcvu3p, xcku040, xczu9eg | golden bitstreams per board part, if the golden designs drive pins | PARTIAL: goldens only possible on xcku3p/xcku5p (and xczu3eg if licensed); other parts get bit meanings by tile type from xcku3p, not same-part segbits |

### E. STA and timing

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Fabric delays: report_timing + write_sdf on real designs | xcku3p | DeepSeek TIMING_XCKU3P unknowns; US+ timing | COVERED |
| Fabric delays: report_timing + write_sdf on real designs | xczu3eg | M5 submitted on xcku3p (Nov 26, redirected) | IN FLIGHT on xcku3p; the xczu3eg run waits on the license probe. If xczu3eg is unlicensed, its measured delays stay blocked and xcku3p values can only be labeled estimates, never claimed as measured. |
| Fabric delays: report_timing + write_sdf on real designs | xcvu3p, xczu9eg, xcku040 | M6 submitted on xcku3p (Nov 26) | BLOCKED for measured delays: all three parts are unlicensed, and delays do not carry across parts (see section 5). The xcku3p run yields xcku3p delays only. |
| Clock delays: BUFGCE/BUFGCTRL/MMCM/PLL insertion delay and skew | xcku3p | CLK_FUZZ_JOB + MMCME4 sweep; US+ timing | COVERED for MMCM/BUFGCE; PLL insertion delay has no named sweep (see F) |
| BRAM/DSP/URAM clock-to-out and setup/hold | xcku3p | TIMING_XCKU3P unknowns | COVERED if the timing designs exercise BRAM, DSP and URAM paths |

### F. Bitgen encodings (per-block)

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| DSP48E2 config-bit encoding (MISSING in both public sources) | xcku3p | DeepSeek DSP48E2_SEARCH | COVERED |
| DSP48E2 config-bit encoding | xcvu3p, xczu9eg, xcku040, xczu3eg | M7 submitted on xcku3p (Nov 26) | IN FLIGHT on xcku3p; encodings carry by DSP48E2 tile type (see section 5). No same-part search is possible on the unlicensed parts. |
| URAM288 content encoding (MISSING in both public sources) | xcku3p | DeepSeek URAM288_SEARCH | COVERED |
| URAM288 content encoding | xcvu3p, xczu9eg, xcku040, xczu3eg | M7 submitted on xcku3p (Nov 26) | IN FLIGHT on xcku3p; encodings carry by URAM288 tile type (see section 5). No same-part search is possible on the unlicensed parts. |
| CLK / BUFGCE / CMT site-config encodings (GAP-B5 remainder, Job 4) | xcku3p | DeepSeek CLK_FUZZ_JOB | COVERED |
| MMCME4 encodings | xcku3p | DeepSeek MMCME4 sweep | COVERED |
| PLL site-config encodings (site_type_PLL.json has pins only) | xcku3p | M8 submitted on xcku3p (Nov 26) | IN FLIGHT on xcku3p; carries by PLL tile type (see section 5) |
| LUTRAM table and logic-location data (GAP-B6; paper V-B "same bits" claim rejected) | xcku3p | none named; M4 not in the Nov 26 wave | MISSED |
| SRL16E/SRL32E encodings (live in SLICEM, same table family as LUTRAM) | xcku3p | none named; M4 not in the Nov 26 wave | MISSED |
| CARRY8 tier 2: golden bitstreams with chained CARRY8 (tier 1 is critic-passed, tier 2 open) | xcku3p | golden bitstreams per board part | PARTIAL: goldens only possible on xcku3p/xcku5p (and xczu3eg if licensed); xcvu3p/xczu9eg/xcku040 tier 2 can only be carry-over |
| FF INIT polarity direct proof (V0 = INIT 0 vs V1 = INIT 1 is resolved on balance of evidence only): a Vivado design with known FF INIT values, bitstream readback | xczu3eg (proving part) or xcku3p | none named; M3 not in the Nov 26 wave | MISSED (provable on xcku3p; carries by SLICEL/SLICEM tile type once run) |
| BRAM tier 2: golden bitstreams with BRAM content (tier 1 already green) | xcku3p | golden bitstreams per board part | PARTIAL: goldens only possible on licensed parts; other parts get carry-over only |
| 54,907 non-band unexplained corpus bits (holes H-2 to H-10) | xcku3p | xcku3p fuzz corpus + goldens | PARTIAL: xcku3p only, and only for features the corpus exercises |
| HDIO/HPIO IOB encodings per part (see D) | xcku3p, xcvu3p | goldens, if they drive pins | PARTIAL: goldens only on licensed parts |

### G. End to end and Tier 2

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Golden bitstreams per board part, with a design list that exercises every block: blinky (LUT/FF/IO), counter (CARRY8 chains), BRAM-content design, DSP design, URAM design, clocking design (MMCM/BUFGCE), LUTRAM design, IOB-heavy design | all five | DeepSeek golden bitstreams per board part | COVERED for xcku3p/xcku5p only (and xczu3eg if licensed). IMPOSSIBLE for xcvu3p, xczu9eg, xcku040: all three are unlicensed, no Vivado bitstream can be built there. The design list still applies to the licensed parts. |
| Tier 2 zero-diff vs a real Vivado bitstream, per block | xcku3p first, then xcvu3p | goldens + fuzz corpus | COVERED for xcku3p (in flight). xcvu3p tier 2 via a same-part golden is impossible; only carry-over from xcku3p tile-type encodings. |
| xczu3eg end-to-end ground truth: Vivado bitstream for the same blinky/counter our flow already emits, to diff against | xczu3eg | goldens per board part, if xczu3eg is one of the board parts | PARTIAL (contingent on the xczu3eg license probe; if licensed, xczu3eg joins the golden set) |
| Control-set ground truth: report_control_sets on Vivado designs, to validate our control-set counting | xcku3p | none named | MISSED (low priority) |

### H. Board and PS

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| ZCU102 PS preset (PSU configuration for the xczu9eg board) | xczu9eg | DeepSeek ZCU102 PS preset | PARTIAL (license flag): xczu9eg is NOT licensed, so any Vivado run the preset job needs is blocked. Confirm with DeepSeek whether the preset can come from board files alone without a licensed part, or whether it is blocked. |
| xczu3eg PS preset (PSU configuration for the proving part) | xczu3eg | ZCU102 preset is xczu9eg only; no xczu3eg preset named | MISSED (contingent on the xczu3eg license probe; if xczu3eg is unlicensed, M9 is blocked with no carry-over: PS is per part) |
| Board part files: which part string each board uses (already known for ZCU102; confirm for the rest when goldens are built) | all five | golden bitstreams per board part | COVERED if the job records the part string per board |

## 2. The missing list: run these before cluster time ends

In priority order. M1-M3 are the three that block named lanes today.
Statuses updated Nov 26, 2026 (evening) for the U1 licensing probe and
the new submission wave.

- M1. IO_CONFIG 55 bits on xcku3p (GAP-B2): Q6 Vivado PinPlanning probe.
  SUBMITTED on xcku3p Nov 26, in flight. Bit meanings carry by HPIO/HDIO
  tile type (see section 5).
- M2. INT_INTF_R_PCIE4 PIP segbits (B1): Vivado designs routing through
  INT_INTF_R_PCIE4 PIPs, bitstream readback. SUBMITTED on xcku3p Nov 26,
  in flight. Segbits carry to any part with the same tile type. Unblocks
  the IOB placement lane.
- M3. FF INIT polarity direct proof: Vivado design with known FF INIT
  values on xczu3p or xcku3p, bitstream readback. NOT in the Nov 26 wave,
  still missed. Provable on xcku3p; carries by SLICEL/SLICEM tile type
  once run. Settles V0/V1 once.
- M4. LUTRAM table + SRL16E/SRL32E encodings (GAP-B6): Vivado
  logic-location data on xcku3p. NOT in the Nov 26 wave, still missed.
- M5. xczu3eg fabric delays: report_timing + write_sdf on xczu3eg
  designs. SUBMITTED on xcku3p Nov 26 (redirected from xczu3eg). The
  xczu3eg run waits on the license probe; if xczu3eg is unlicensed, its
  measured delays stay blocked. Only BRAM 1020 ps is documented today.
- M6. Fabric delays per part: report_timing + write_sdf on xcvu3p,
  xczu9eg, xcku040. SUBMITTED on xcku3p Nov 26, but measured delays for
  xcvu3p, xczu9eg, and xcku040 are PROVEN BLOCKED: all three parts are
  unlicensed, and delays do not carry across parts (see section 5). The
  xcku3p run yields xcku3p delays only. For the unlicensed parts the
  timing DB stays Ballpark/UNKNOWN, or uses xcku3p values as labeled
  estimates, never as measured.
- M7. DSP48E2/URAM288/CLK fuzz per part: SUBMITTED on xcku3p Nov 26.
  Encodings carry by tile type (see section 5). Per-part fuzz corpora for
  the unlicensed parts remain unbuildable.
- M8. PLL site-config encodings: a PLL sweep to match the MMCME4 sweep.
  SUBMITTED on xcku3p Nov 26, in flight. Carries by PLL tile type.
- M9. xczu3eg PS preset: PSU configuration for the proving part. Still
  missed, contingent on the xczu3eg license probe; if xczu3eg is
  unlicensed, blocked with no carry-over (PS is per part).
- M10. Inference ground truth: Vivado synth of multiply, arithmetic and
  memory patterns, to settle DSP48E2/CARRY8/LUTRAM inference (B).
  SUBMITTED on xcku3p Nov 26, in flight. Mapping rules carry by
  primitive, not by part (see section 5).
- M11. CLOSED Nov 26: device dumps for every package of every UltraScale
  and UltraScale+ family, plus primitive properties for xcvu3p and
  xczu3eg (cluster report). Follow-up: the dump files still need to land
  in the repo as data files before dependent rows can cite them.
- M12. xcvu3p fuzz corpus: PROVEN BLOCKED. xcvu3p is unlicensed; no
  bitstream can be built there, so no same-part corpus is possible.
  Redirect: the corpus is xcku3p/xcku5p-based, and xcvu3p INT PIP
  completeness comes from carried-over xcku3p encodings plus the M11
  dumps, not from a same-part corpus.

## 3. Per-part matrix

| Part | Device model | Synth props | Pack/place geom | Route (INT PIPs) | Route (INT_INTF) | Timing | Bitgen encodings | Goldens | PS preset |
|---|---|---|---|---|---|---|---|---|---|
| xcku3p | COVERED (M11) | IN FLIGHT (M10 on xcku3p) | COVERED (M11) | COVERED (fuzz corpus) | IN FLIGHT (M2) | COVERED | IN FLIGHT (M1/M2/M7/M8 submitted; M3/M4 missed) | COVERED if design list is full | n/a |
| xcvu3p | COVERED (M11) | COVERED (M11) + inference IN FLIGHT (carries) | COVERED (M11) | CARRY-OVER ONLY (M12 blocked) | PARTIAL (tileconn + M2 carry-over) | BLOCKED (no measured delays possible) | CARRY-OVER (tile-type encodings from xcku3p) | IMPOSSIBLE (unlicensed) | n/a |
| xczu3eg | COVERED (M11) | COVERED (M11) + inference IN FLIGHT (carries) | COVERED (M11) | COVERED (public segbits + tileconn) | COVERED (public tileconn; blinky flows) | CONTINGENT (probe; M5 redirected to xcku3p) | PARTIAL (LUT/FF/BRAM green; M3/M4 missed) | CONTINGENT (probe) | CONTINGENT (probe) |
| xczu9eg | COVERED (M11) | COVERED (M11) + inference IN FLIGHT (carries) | COVERED (M11) | CARRY-OVER ONLY (no corpus) | PARTIAL (tileconn + M2 carry-over) | BLOCKED (no measured delays possible) | CARRY-OVER (tile-type encodings from xcku3p) | IMPOSSIBLE (unlicensed) | PARTIAL (license flag) |
| xcku040 | COVERED (M11) | COVERED (M11) + inference IN FLIGHT (carries) | COVERED (M11) | CARRY-OVER ONLY (no corpus) | PARTIAL (tileconn + M2 carry-over) | BLOCKED (no measured delays possible) | CARRY-OVER (tile-type encodings from xcku3p) | IMPOSSIBLE (unlicensed) | n/a |
| xcku5p | PARTIAL (M11 dumps exist on cluster; no xcku5p.json in repo yet) | COVERED (M11 dumps, file pending) | PARTIAL (dumps exist; device file pending) | COVERED (licensed; corpus possible) | IN FLIGHT (M2 carries) | COVERED (licensed; runs possible) | IN FLIGHT (same wave as xcku3p) | COVERED if design list is full | n/a |

## 4. Notes

- The vendored public data is zynqusp-only. Every xcku3p/xcvu3p segbits
  fact must come from Vivado fuzz or goldens; there is no public DB to
  fall back on for those parts.
- The part access probe answers "can Vivado run on this part", not "do we
  have the encodings". Do not treat a passing probe as coverage of any
  row above. U1 has now run (see Licensing reality); the probe itself is
  done, the encodings still have to land per item.
- The golden-bitstream job is the single highest-leverage job: one golden
  per block per part closes tier 2 for that block, but only if the design
  list in section 1G is what gets built. Blinky-only goldens leave
  CARRY8, BRAM, DSP, URAM, LUTRAM and IOB tier 2 open. Goldens are only
  possible on licensed parts (xcku3p, xcku5p, and xczu3eg if its probe
  passes).
- xcku5p is licensed but has no device file in the repo yet (no
  xcku5p.json). If goldens or fuzz come from xcku5p, a device file is
  needed first; the M11 dumps are the source for it.
- When a missing item lands, record it the way the public data is
  recorded: source (Vivado 2017.3, design name, command), part, date, and
  the file it lands in. Never invent bits, FARs, or delays.

## 5. Carry-over: what xcku3p facts transfer to xcvu3p, xczu9eg, and xcku040

Principle: a bitstream encoding lives in the tile type, not in the part.
The UltraScale+ family shares the INT, CLE, BRAM, and DSP tile types
across parts, so a config-bit encoding fuzzed on xcku3p is the same
encoding on xcvu3p, xczu9eg, or xcku040 wherever the same tile type
exists. What differs per part is the tilegrid (where the tiles sit), the
package (which pins exist), the clocking topology, and the timing (how
long the routes are). So encodings carry; geometry, pins, clocks, and
delays do not.

Verification rule: a carried fact is usable on a target part only if the
M11 dumps confirm the tile type exists there with the same site types.
Cite the dump as the source. Never carry a fact to a part whose dumps
have not been checked for that tile type.

### Carries (xcku3p to xcvu3p, xczu9eg, xcku040, by tile type)

- INT PIP segbits (INT tile type; the public INT DB is already per tile
  type). Covers the D row and the M12 redirect.
- CLE encodings (SLICEL/SLICEM): LUT_INIT, FF_INIT (including polarity
  once M3 proves it), CARRY8 config bits.
- LUTRAM and SRL16E/SRL32E encodings (SLICEM), once M4 lands.
- BRAM content and parity encodings (RAMB36E2/RAMB18E2).
- DSP48E2 config-bit encodings (M7).
- URAM288 content encodings (M7). Evidence in the repo: xcku3p.json and
  xcvu3p.json both list URAM_URAM_FT and URAM_URAM_DELAY_FT tile types
  with URAM288 sites. Caveat: xcvu3p.json tile pairs were copied from
  the xcku3p template, so confirm URAM tile presence on xcvu3p (and on
  xczu9eg/xcku040) from the M11 dumps before relying on it.
- CLK/HDISTR/clock-spine bit meanings and BUFGCE/BUFGCTRL/MMCM
  site-config bits (per CLK/CMT tile type). The spine reach and clock
  region layout do not carry; only the per-tile bit meanings do.
- PLL site-config bits (M8), per PLL tile type.
- IO_CONFIG bit meanings (M1's 55 bits), per HPIO/HDIO tile type. Only
  the meaning of each bit carries; which IOB sites exist and the pin
  mapping are per package and do not carry.
- INT_INTF PIP segbits (M2), per INT_INTF tile type, where the target
  part has the same tile type (for example INT_INTF_R_PCIE4). Confirm
  the tile type exists in the target part from the M11 dumps.
- Inference ground truth (M10): which RTL patterns Vivado maps to
  DSP48E2, CARRY8, or LUTRAM is a synthesis-mapping fact per primitive,
  not per part, so the mapping rules carry. Resource-driven choices
  (DSP vs LUTs under utilization pressure) can still differ per part.
- Control-set counting rules (UG574): family-level, carries.

### Does not carry (per part, must come from that part's own data)

- Frame addresses, FAR layout, and frame counts (per-part tilegrid;
  xcku3p frame sum 41,460 vs xcvu3p 73,035 constructed).
- Tilegrid: column order and positions of INT, INT_INTF, CLE, BRAM,
  DSP, and URAM columns; which columns exist at all.
- Package pins and pinout (per package; the M11 dumps are the source).
- I/O bank placement and HDIO vs HPIO bank locations.
- Clock regions, CMT column placement, and BUFGCE/BUFGCTRL site counts
  and positions.
- PS and PS presets (per Zynq part; no carry). This is why M9 has no
  fallback if xczu3eg is unlicensed.
- Hard blocks: GTY/GTH transceivers, PCIe blocks, Ethernet (per part).
- Fabric timing delays (per part and per speed grade; routing lengths
  differ). This is why M6 is proven blocked for the unlicensed parts.
- Device capacity and resource counts.
- Bitstream size and IDCODE.
- Fuzz corpora and golden bitstreams (must be built per part; a corpus
  on xcku3p does not exercise xcvu3p-specific tiles).
- Slice counts and frame totals (the xcku3p 20,340 vs 27,120 dispute is
  xcku3p-specific).

### Second licensed source

xcku5p is licensed too, so goldens, timing, and fuzz can also come from
xcku5p. It shares the same tile types, so its encodings cross-check the
xcku3p ones. It needs a device file in the repo first (no xcku5p.json
yet); the M11 dumps are the source for it.
