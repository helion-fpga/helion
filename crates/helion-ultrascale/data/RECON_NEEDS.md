# RECON_NEEDS: every Vivado fact the UltraScale+ flow still needs

Date: 2026-11-26. Vivado 2017.3 is up on the USF cluster right now.
DeepSeek is running the UltraScale recon; Claude Code is dumping device
data. This file lists every Vivado fact the Helion UltraScale+ flow still
needs for synthesis, placement, routing and bitgen, per part and per board,
and marks each one COVERED (a running recon job produces it), PARTIAL (a
running job covers it only in part or only for one part) or MISSED (no
running job produces it; run it before cluster time ends).

Scope rule from the goal: one-time Vivado use for fuzzing DB generation is
allowed; the shipped tool stays license-free. Nothing in this file asks
Vivado to do the flow for us. Every item is a fact (a table, an encoding,
a delay, a bitstream) that Helion then consumes from its own data files.
No merges; this file lives on its own fm/ branch.

Parts: xcku3p (primary, xcku3p-ffva676-1-e), xcvu3p (xcvu3p-ffvc1517-1-e),
xczu3eg (proving part, end to end flows today), xczu9eg (ZCU102),
xcku040 (new). Boards: ZCU102 is xczu9eg. Other eval boards are TBD in
this file; the golden-bitstream job is per board part, so the part is the
unit that matters.

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
| Tilegrid: tile types, locations, FAR layout per part | xcku3p, xcvu3p, xczu9eg, xcku040, xczu3eg | Claude Code tiles dump (xcku3p, xczu9eg, xcku040); DeepSeek tile templates | PARTIAL: xcvu3p and xczu3eg tilegrids have no named dump |
| Site models: site types, pins, site PIPs per part | xcku3p, xcvu3p, xczu9eg, xcku040, xczu3eg | Claude Code site models (xcku3p, xczu9eg, xcku040) | PARTIAL: xcvu3p and xczu3eg site models have no named dump |
| Package pins: pin, bank, I/O standard, signal per part | xcku3p, xcvu3p, xczu9eg, xcku040, xczu3eg | Claude Code package pins (xcku3p, xczu9eg, xcku040) | PARTIAL: xcvu3p and xczu3eg package pins have no named dump |
| xcvu3p frame total (US-06, GAP-F9): UG570 says 71820, our file constructs 73035, overshoot 1215 | xcvu3p | DeepSeek DEVICE_MODEL_GAPS xcvu3p | COVERED |
| xcvu3p slice count: AMD XMP103 says 49,250, our file says 49,260 | xcvu3p | DeepSeek DEVICE_MODEL_GAPS xcvu3p | COVERED |
| xcku3p slice count: AMD says 20,340, our file says 27,120; frame sum 41460 vs UG570 41476 (16 frames) | xcku3p | DeepSeek DEVICE_MODEL_GAPS is xcvu3p only; Claude Code tiles for xcku3p give geometry | PARTIAL: the dispute needs a Vivado tilegrid read on xcku3p, which the tiles dump provides; no named job checks the counts against it |
| xcvu3p FAR rows, clock-region rows/cols, bram/dsp col majors: all constructed in our file, none Vivado-confirmed | xcvu3p | DeepSeek DEVICE_MODEL_GAPS xcvu3p | COVERED |
| Per-part IDCODE and frame layout (FAR rows, frame counts) for xcku040 and xczu9eg | xcku040, xczu9eg | Claude Code tiles + DeepSeek golden bitstreams per board part | COVERED if the goldens include xcku040 and xczu9eg |
| Part access: license checkout works for each part (probe, not a claim) | all five | DeepSeek part access probe | COVERED |

### B. Synthesis mapping

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Primitive properties: INIT values, IS_*_INVERTED defaults, property ranges per primitive | all five | Claude Code primitive properties (xcku3p, xczu9eg, xcku040) | PARTIAL: xcvu3p and xczu3eg have no named dump |
| DSP48E2 inference ground truth: which multiply/add patterns Vivado maps to DSP48E2, with which attributes | xcku3p | DSP48E2_SEARCH is config-bit encoding, not inference | MISSED |
| CARRY8 inference ground truth: which arithmetic patterns become chained CARRY8 | xcku3p | none named | MISSED |
| LUTRAM/SRL inference ground truth: which patterns become LUTRAM vs SRL16E/SRL32E vs LUTs | xcku3p | none named | MISSED |

### C. Pack and place

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Tile geometry for legality: CLB column layout, DSP/BRAM/URAM column positions, clock-region boundaries | all five | Claude Code tiles + site models; DeepSeek tile templates | PARTIAL: xcvu3p and xczu3eg geometry has no named dump |
| Control-set limits (max unique control sets per slice, per UG574) | family | none named; doc knowledge, not Vivado data | MISSED (low priority: read UG574, no cluster time needed) |
| CARRY8 chain placement: vertically contiguous slice columns that accept chains | xcku3p | tile templates give geometry | COVERED if the templates include CLB column adjacency |
| IO/DSP/BRAM column legality vs tilegrid | xcku3p, xcvu3p | Claude Code tiles (xcku3p) + DEVICE_MODEL_GAPS (xcvu3p) | PARTIAL: xcvu3p tile dump not named |

### D. Routing

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| INT PIP segbits completeness per part (public INT DB is zynqusp only) | xcku3p, xcvu3p | DeepSeek xcku3p fuzz corpus (single-feature/pair sweeps) | PARTIAL: xcvu3p has no named fuzz corpus |
| INT_INTF tile wiring map (BEL-pin-to-INT-wire for DSP/BRAM/URAM pins) | xcku3p, xcvu3p | DeepSeek tileconn; Claude Code tiles + site models | PARTIAL: xczu3eg map is public tileconn.json; xcku3p/xcvu3p maps come from the recon tileconn dump |
| INT_INTF_R_PCIE4 PIP segbits (B1, the IOB placement lane blocker; emitter fails loud UnknownFeature, zero public files contain IMUXOUT16) | xcku3p (and any part whose IOB routes through INT_INTF_R_PCIE4) | none named; tileconn is wiring, not segbits | MISSED |
| IO_CONFIG 55 bits (GAP-B2, Q6 Vivado PinPlanning probe) | xcku3p | none named; package pins dump is pins, not the PinPlanning probe | MISSED |
| HDIO/HPIO IOB segbits per part (public HDIO/HPIO DBs are zynqusp only) | xcku3p, xcvu3p, xcku040, xczu9eg | golden bitstreams per board part, if the golden designs drive pins | PARTIAL: no dedicated IOB fuzz job named |

### E. STA and timing

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Fabric delays: report_timing + write_sdf on real designs | xcku3p | DeepSeek TIMING_XCKU3P unknowns; US+ timing | COVERED |
| Fabric delays: report_timing + write_sdf on real designs | xczu3eg | TIMING_XCKU3P is xcku3p only; US+ timing is family-level, no xczu3eg run named | MISSED |
| Fabric delays: report_timing + write_sdf on real designs | xcvu3p, xczu9eg, xcku040 | none named per part | MISSED |
| Clock delays: BUFGCE/BUFGCTRL/MMCM/PLL insertion delay and skew | xcku3p | CLK_FUZZ_JOB + MMCME4 sweep; US+ timing | COVERED for MMCM/BUFGCE; PLL insertion delay has no named sweep (see F) |
| BRAM/DSP/URAM clock-to-out and setup/hold | xcku3p | TIMING_XCKU3P unknowns | COVERED if the timing designs exercise BRAM, DSP and URAM paths |

### F. Bitgen encodings (per-block)

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| DSP48E2 config-bit encoding (MISSING in both public sources) | xcku3p | DeepSeek DSP48E2_SEARCH | COVERED |
| DSP48E2 config-bit encoding | xcvu3p, xczu9eg, xcku040, xczu3eg | searches named without a part; assume xcku3p unless the job covers all parts | PARTIAL (ambiguous: confirm the search covers every part, or run it per part) |
| URAM288 content encoding (MISSING in both public sources) | xcku3p | DeepSeek URAM288_SEARCH | COVERED |
| URAM288 content encoding | xcvu3p, xczu9eg, xcku040, xczu3eg | same ambiguity as DSP48E2 | PARTIAL (ambiguous: confirm per-part coverage) |
| CLK / BUFGCE / CMT site-config encodings (GAP-B5 remainder, Job 4) | xcku3p | DeepSeek CLK_FUZZ_JOB | COVERED |
| MMCME4 encodings | xcku3p | DeepSeek MMCME4 sweep | COVERED |
| PLL site-config encodings (site_type_PLL.json has pins only) | xcku3p | MMCME4 sweep is MMCM only; no PLL sweep named | MISSED |
| LUTRAM table and logic-location data (GAP-B6; paper V-B "same bits" claim rejected) | xcku3p | none named | MISSED |
| SRL16E/SRL32E encodings (live in SLICEM, same table family as LUTRAM) | xcku3p | none named | MISSED |
| CARRY8 tier 2: golden bitstreams with chained CARRY8 (tier 1 is critic-passed, tier 2 open) | xcku3p | golden bitstreams per board part | PARTIAL: covered only if a golden design chains CARRY8; no dedicated CARRY8 sweep named |
| FF INIT polarity direct proof (V0 = INIT 0 vs V1 = INIT 1 is resolved on balance of evidence only): a Vivado design with known FF INIT values, bitstream readback | xczu3eg (proving part) or xcku3p | none named | MISSED |
| BRAM tier 2: golden bitstreams with BRAM content (tier 1 already green) | xcku3p | golden bitstreams per board part | PARTIAL: covered only if a golden design writes BRAM content |
| 54,907 non-band unexplained corpus bits (holes H-2 to H-10) | xcku3p | xcku3p fuzz corpus + goldens | PARTIAL: xcku3p only, and only for features the corpus exercises |
| HDIO/HPIO IOB encodings per part (see D) | xcku3p, xcvu3p | goldens, if they drive pins | PARTIAL |

### G. End to end and Tier 2

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| Golden bitstreams per board part, with a design list that exercises every block: blinky (LUT/FF/IO), counter (CARRY8 chains), BRAM-content design, DSP design, URAM design, clocking design (MMCM/BUFGCE), LUTRAM design, IOB-heavy design | all five | DeepSeek golden bitstreams per board part | COVERED if the design list above is what the job builds; PARTIAL if the goldens are blinky-only |
| Tier 2 zero-diff vs a real Vivado bitstream, per block | xcku3p first, then xcvu3p | goldens + fuzz corpus | COVERED for xcku3p; PARTIAL for xcvu3p (no named fuzz corpus) |
| xczu3eg end-to-end ground truth: Vivado bitstream for the same blinky/counter our flow already emits, to diff against | xczu3eg | goldens per board part, if xczu3eg is one of the board parts | PARTIAL (ambiguous: confirm xczu3eg is in the golden set) |
| Control-set ground truth: report_control_sets on Vivado designs, to validate our control-set counting | xcku3p | none named | MISSED (low priority) |

### H. Board and PS

| Need | Parts | Recon coverage | Status |
|---|---|---|---|
| ZCU102 PS preset (PSU configuration for the xczu9eg board) | xczu9eg | DeepSeek ZCU102 PS preset | COVERED |
| xczu3eg PS preset (PSU configuration for the proving part) | xczu3eg | ZCU102 preset is xczu9eg only; no xczu3eg preset named | MISSED |
| Board part files: which part string each board uses (already known for ZCU102; confirm for the rest when goldens are built) | all five | golden bitstreams per board part | COVERED if the job records the part string per board |

## 2. The missing list: run these before cluster time ends

In priority order. M1-M3 are the three that block named lanes today.

- M1. IO_CONFIG 55 bits on xcku3p (GAP-B2): Q6 Vivado PinPlanning probe.
  Unblocks the IO_CONFIG block. Nothing in the recon names it.
- M2. INT_INTF_R_PCIE4 PIP segbits (B1): Vivado designs routing through
  INT_INTF_R_PCIE4 PIPs, bitstream readback. Unblocks the IOB placement
  lane. Tileconn gives wiring, not segbits; no named job covers it.
- M3. FF INIT polarity direct proof: Vivado design with known FF INIT
  values on xczu3p or xcku3p, bitstream readback. Settles V0/V1 once.
- M4. LUTRAM table + SRL16E/SRL32E encodings (GAP-B6): Vivado
  logic-location data on xcku3p. No named job.
- M5. xczu3eg fabric delays: report_timing + write_sdf on xczu3eg
  designs. Only BRAM 1020 ps is documented today.
- M6. Fabric delays per part: report_timing + write_sdf on xcvu3p,
  xczu9eg, xcku040. Only xcku3p is named.
- M7. DSP48E2/URAM288/CLK fuzz per part: confirm the three searches cover
  xcvu3p, xczu9eg, xcku040 and xczu3eg, or run them per part. Right now
  they are named without a part.
- M8. PLL site-config encodings: a PLL sweep to match the MMCME4 sweep.
- M9. xczu3eg PS preset: PSU configuration for the proving part.
- M10. Inference ground truth: Vivado synth of multiply, arithmetic and
  memory patterns, to settle DSP48E2/CARRY8/LUTRAM inference (B).
- M11. xcvu3p and xczu3eg tiles, site models, package pins and primitive
  properties: Claude Code covers xcku3p, xczu9eg, xcku040 only. Extend
  the dumps to the other two parts.
- M12. xcvu3p fuzz corpus: xcku3p has one; xcvu3p has none named. Needed
  for INT PIP segbits completeness and the 54,907-bit class on xcvu3p.

## 3. Per-part matrix

| Part | Device model | Synth props | Pack/place geom | Route (INT PIPs) | Route (INT_INTF) | Timing | Bitgen encodings | Goldens | PS preset |
|---|---|---|---|---|---|---|---|---|---|
| xcku3p | PARTIAL (tiles, site models, pins covered; slice/frame disputes need the count check) | PARTIAL (props covered; inference missed) | PARTIAL | PARTIAL (fuzz corpus covers) | MISSED (B1 segbits) | COVERED | PARTIAL (DSP/URAM/CLK/MMCM covered; LUTRAM/SRL/PLL/FF-INIT missed) | COVERED if design list is full | n/a |
| xcvu3p | COVERED (DEVICE_MODEL_GAPS) | PARTIAL (no props dump named) | PARTIAL (no tile dump named) | MISSED (no fuzz corpus) | PARTIAL (tileconn dump) | MISSED | PARTIAL (searches ambiguous per part) | PARTIAL (goldens per part; fuzz missing) | n/a |
| xczu3eg | PARTIAL (no tile/site/pin dump named) | PARTIAL (no props dump named) | PARTIAL | COVERED (public segbits + tileconn) | COVERED (public tileconn; blinky flows) | MISSED (only BRAM 1020 ps) | PARTIAL (LUT/FF/BRAM green; LUTRAM/FF-INIT missed) | PARTIAL (confirm in golden set) | MISSED |
| xczu9eg | COVERED (tiles, site models, pins) | PARTIAL (props covered; inference missed) | COVERED | PARTIAL (no fuzz corpus named) | PARTIAL (tileconn dump) | MISSED | PARTIAL (searches ambiguous per part) | COVERED if in golden set | COVERED (ZCU102) |
| xcku040 | COVERED (tiles, site models, pins) | PARTIAL (props covered; inference missed) | COVERED | MISSED (no fuzz corpus) | PARTIAL (tileconn dump) | MISSED | PARTIAL (searches ambiguous per part) | COVERED if in golden set | n/a |

## 4. Notes

- The vendored public data is zynqusp-only. Every xcku3p/xcvu3p segbits
  fact must come from Vivado fuzz or goldens; there is no public DB to
  fall back on for those parts.
- The part access probe answers "can Vivado run on this part", not "do we
  have the encodings". Do not treat a passing probe as coverage of any
  row above.
- The golden-bitstream job is the single highest-leverage job: one golden
  per block per part closes tier 2 for that block, but only if the design
  list in section 1G is what gets built. Blinky-only goldens leave
  CARRY8, BRAM, DSP, URAM, LUTRAM and IOB tier 2 open.
- When a missing item lands, record it the way the public data is
  recorded: source (Vivado 2017.3, design name, command), part, date, and
  the file it lands in. Never invent bits, FARs, or delays.
