# xczu3eg emitter-device lane: blinky emits a real bitstream on xczu3eg

Date: 2026-11-22. Lane branch fm/ultra-bitgen-emitter-xczu3eg, tip
a8817087 (merge of W-DEVICE ba19b579 and W-FLOW-2 76952928 onto
GAP-F5 base 0ba230af). Worktree
~/workspace/helion-ultra-bitgen-emitter-xczu3eg. No merges into
fm/ultra-bitstream, fm/zedboard-merge, or master. No em dashes in
this report (verified by grep). Workstream reports:
~/workspace/ultra-bitgen-crew/FLOW_XCZU3EG_REPORT.md (W-FLOW-2,
237 lines). Logs: ~/workspace/emitter-xczu3eg-logs/ (W-DEVICE base
and after logs, P-13 header bytes; flow/ partial W-FLOW-1 log;
flow2/ complete W-FLOW-2 logs; merged/ merged-branch chain logs).

## Headline

Blinky emits a real bitstream on xczu3eg end to end: `helion impl
examples/blinky.sv --part xczu3eg` with the tileconn map exits 0
and writes blinky.xczu3eg.bit, 5,982,532 bytes, 14,952 FDRI
frames, IDCODE 0x04a42093. The emitted bytes decode back with
exactly 38 unexplained set bits, all of them the H-1 band rule and
nothing else. The bitstream is NOT verified end to end: the US-09
checkers FAIL on route and cosim with a single pinned root cause,
the flow emits no FFMUXA1 line, plus a pre-existing FF INIT
polarity disagreement. The lane proves the emitter-device path
works; it does not prove a board-ready blinky. Do not claim US-09
PASS. No bitstream existed on xczu3eg before this lane; the
furthest GAP-F5 run stopped at BITGEN.

## 1. What changed (two workstreams, both merged --no-ff)

- W-DEVICE ba19b579 (1 file, crates/helion-cli/src/ultra.rs,
  +139/-7): "xczu3eg" arm in load_ultra_device loading
  data/xczu3eg.json via DeviceFile::from_path, mirroring the
  xcku3p arm including its error mapping. Catch-all error now
  reads "only xcku3p and xczu3eg are wired". xcvu3p still fails
  loud with the provisional message.
- W-FLOW-2 76952928 (1 file, crates/helion-cli/src/ultra.rs,
  +154, test only): regression test
  blinky_flow_emits_bitstream_on_xczu3eg.
- No other files changed. No decoder, emitter, segbits, or
  frame-total code touched.

## 2. IDCODE 0x04a42093 (verified, not invented)

- Read from data/xczu3eg.json (slrs.SLR0.idcode); DeviceFile
  needs nothing supplied.
- Independently verified by W-DEVICE against P-13
  cnvW1A1-ultra96.bit (md5 a56a86cf137c719c09f891a8b2472280
  matches the corpus manifest; header part
  xczu3eg-sbva484-1-i; bytes at offset 291 are 04 a4 20 93).
  The coordinator re-read those header bytes itself and confirmed
  04a42093. The decoder's embedded geometry table
  (bitstream_packets.rs) independently carries the same value.
- UG570 Table 2 does not list Zynq MPSoC parts, so the corpus
  bitstream is the authoritative source. The AMD forum figure
  0x14710093 is the PS TAP JTAG IDCODE, not the PL configuration
  IDCODE. The emitted blinky bitstream contains 0x04a42093
  (IDCODE register write reads it back; raw bytes found at offset
  68).

## 3. Frame total: 14,952 FDRI frames, not 14,964

- DeviceFile::total_frames() for xczu3eg is 14,952 (4,984 per FAR
  row x 3 rows). emit_bitstream writes one single-frame FDRI
  packet per FAR in config order. The emitted blinky bitstream
  carries 14,952 FDRI frames, counted two ways by W-FLOW-2
  (packet walk: FDRI words 1,390,536 / 93 = 14,952; CRC words
  also 14,952) and confirmed by the coordinator's own packet
  walk (FDRI words 1,390,536, frames 14,952). The 12-frame gap
  vs UG1085 Table 1 (ZU3) 14,964 is end-of-row padding Vivado
  carries and the decoder skips (padding_skipped=12), the same
  pattern as xcku3p (41,460 emitted vs 41,476 UG570). No
  frame-total pinning or decoder code was touched.

## 4. Blinky flow run (exit 0)

Command (from the flow worktree, release CLI):

```sh
HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp \
HELION_TILECONN_PART=xczu3eg-sbva484-1-e \
./target/release/helion impl examples/blinky.sv --part xczu3eg
```

- blinky.xczu3eg.bit, 5,982,532 bytes; blinky.xczu3eg.fasm, 13
  lines, byte-identical to
  ~/workspace/gapf5-logs/blinky.xczu3eg.merged.fasm (diff empty).
- synth 1 LUT, 1 flop; pack 1 slice; place seed 0; legality 0
  violations; STA WNS_PS=9650, TNS_PS=0, estimate=1; route 2
  nets, 1 iter, 0 overused, 6 PIPs; bitgen parse-back passed
  inside the flow (parse_back_lines=567585).
- INT tiles used: INT_X1Y0 only, all 6 PIPs. The provisional INT
  rule agrees with the tilegrid sidecar table for INT_X1, so
  blinky is unaffected by the INT_X2+ misprediction (section 8).
- IOB coverage: none. Nets q and led skipped (IOB cell u_iob has
  no placed site, the known GAP-F5 gap). The emitted bitstream
  carries no IOB config. Not a board-ready bitstream.

## 5. Decode round-trip (exit 1, explained, not an emitter defect)

Command: `./target/release/helion-bitgen decode --bit
<blinky> --db-root <zynqusp> --part xczu3eg-sbva484-1-e`.
Exit 1 with exactly 38 unexplained set bits, all at absolute
frame bits 1440-1486, inside the H-1 band range 1440-1487, on 6
FARs: 0x30c (10), 0x403 (7), 0x404 (2), 0x407 (7), 0x40d (5),
0x411 (7). Per-FAR counts equal the stored-band popcounts
exactly. The coordinator recounted the 38 lines from the decode
log itself. The decoded FASM (82 lines) contains all 13 flow
lines, verified mechanically (comm: 0 lines missing); the extra
69 lines are default-valued features the emitter writes.

Band check via check_stored_band (the N7 function) over all
14,952 frames with an all-false LUTRAM mask (blinky has no
LUTRAM): the regression test asserts exact == 14,952 and panics
on any Q residual or mismatch, and it passes. The band-check log
prints the 6 nonzero-band FARs, each Exact. Conclusion: the 38
bits are exactly the H-1 band rule and nothing else is
unexplained. The decode exit 1 is a decoder-side gap:
bitstream_decode.rs lists band bits as unexplained and fails
loud, while parse_bitstream (bitgen.rs, N7) applies the band
check and passes. Porting the N7 check into the decoder is a
follow-up, not this lane.

## 6. US-09 checkers: FAIL, root cause pinned

verify-bit on the emitted bitstream: FAIL. decode FAIL (the 38
band bits, section 5); parse ok (2 tiles, 82 features); ports
ok; place ok (1 slice, no violations); route FAIL (undriven
sinks on the A_O and AQ nets; three nets driven only by a PIP);
extract ok (1 site, 18 warnings, all unclassified defaults);
cosim FAIL (0 comparisons, 400 RTL unknowns with
examples/blinky.sv, whose `logic q` is uninitialized; with the
initialized fixture RTL, 400 comparisons, 400 mismatches, 400
net unknowns: extracted AQ is X). The golden fixture passes all
checks, so the checkers work.

Root cause, pinned by controlled experiments on copies of the
decoded FASM: the flow emits no FFMUXA1 line at all (zero
FFMUX hits in fasm_gen.rs and ultra.rs; zero in the emitted
FASM, confirmed by the coordinator). The flow's wire model
routes LUT-O to FF-D through the INT to the AX site pin (the
FFMUXA1.BYP shape), but the FFMUX selection is never written.
The six FFMUXA1 select bits are all clear in the emitted
frames, matching no segbits option, so the FF D mux is
unconfigured. The route checker only counts a slice Q pin as a
driver when an FFMUX feature selects it, and the extractor only
models an FF when an FFMUX feature is present, so both fail.

- Adding FFMUXA1.BYP fixes the AQ violations, but route still
  fails on the A_O net (the checker does not count the LUT O pin
  as a driver) and cosim fails at elaboration (undefined wire
  s_SLICE_X1Y0__A_O). BYP is not modellable by US-09 as written.
- Adding FFMUXA1.D6 gives real values but all 400 inverted
  (rtl=0 net=1, ...). That is the pre-existing FF INIT polarity
  disagreement (INTERFACES.md item 5): the flow writes
  AFF.INIT.V0 meaning INIT 0, while us09v ff_attr reads V0 as
  INIT 1. It needs a Vivado-known-INIT experiment to resolve.

Follow-ups (not this lane): (1) emit FFMUX selections in the
flow FASM and decide BYP vs D6 (the router routes LUT-O to FF-D
through the fabric, i.e. BYP shape, while the fixture uses D6
with no fabric route for that net); (2) port the N7 band check
into bitstream_decode.rs; (3) resolve the FF INIT polarity flag
with a Vivado-known-INIT experiment; (4) teach the US-09
extractor and route checker the BYP-through-fabric path if BYP
is chosen.

## 7. Counter (stretch)

Exit 1, no .bit, exact known GAP-F5 failure unchanged: ROUTE
OveruseTimeout, net cnt_1, wire 36, tile (1,0), 3 nets, 50
iters. Only tile (1,0), i.e. INT_X1Y0, is involved; the router
never reached PIP emission, so no INT_X2+ limitation applies.
Counter does not block the blinky verdict.

## 8. Carried caveats and gaps

- INT_X2+ unproven: load_ultra_device attaches no INT column
  table, so INT tiles resolve by the provisional rule, which
  agrees with the tilegrid table for INT_X1 but mispredicts
  INT_X2 and above per tests/device_file_xczu3eg.rs. Any design
  routing through INT_X2+ on xczu3eg needs set_int_columns with
  xczu3eg_int_columns.json wired in (follow-up).
- IOB cells have no placed site (2 nets skipped per run); no IOB
  config is emitted.
- No documented xczu3eg timing (DS925 TRCKO_DO unsourced);
  STA numbers remain preliminary estimates.
- The decoder needs the N7 band check ported (section 5).
- Flags A-CTRL and A-F7F8 remain unresolved, need a CTO ruling.

## 9. Suites

- Base (GAP-F5 tip 0ba230af): ultrascale 423/0/1, CLI 66/0/1
  (W-DEVICE base verification).
- After W-DEVICE (ba19b579): ultrascale 423/0/1, CLI 69/0/1
  (net +3 CLI tests).
- After W-FLOW-2 (76952928, on the flow branch): ultrascale
  423/0/1, CLI 70/0/1 (the +1 is the new regression test,
  skipped without the tileconn env; exercised with the env it
  passes in 7.98 s and emits a byte-identical 5,982,532-byte
  bitstream).
- Merged lane branch (a8817087): ultrascale 423/0/1
  (ULTRA_EXIT:0, 31 result lines), CLI 70/0/1 (CLI_EXIT:0, 11
  result lines), 0 FAILED lines. Recounted by the coordinator
  from ~/workspace/emitter-xczu3eg-logs/merged/ after the chain
  finished (about 40 minutes). The merge commit adds no code
  over the flow branch tip, so the counts match the flow-branch
  runs exactly, as expected.

## 10. Regression test

blinky_flow_emits_bitstream_on_xczu3eg (crates/helion-cli/src/
ultra.rs): runs run_impl on examples/blinky.sv for xczu3eg and
asserts BITGEN reached, .bit and .fasm written, IDCODE
0x04a42093 present, 14,952 FDRI frames, the exact 13-line
FASM, every PIP on INT_X1Y0, all 14,952 stored bands Exact
under masked MAP-A (a Q residual panics), and the standalone
decoder round-trips all 13 flow lines with exactly 38
unexplained bits, all inside frame bits 1440..=1487. It skips
when HELION_TILECONN_DB_ROOT / HELION_TILECONN_PART are unset.
US-09 verify-bit is deliberately not asserted: it fails on the
FFMUX gap, and asserting it would enshrine a known-bad state.

## 11. Incidents

- W-FLOW-1 (223e3f1d) errored Nov 22, about 01:05 EST on a
  runtime restart drain ("restart drain in progress; rejecting
  new submission"), mid-suite-verification. Coordinator
  investigation: session files last wrote at the error time, the
  polled cargo process was gone, resume failed (terminal
  worker). Only a partial suite log survived (lib target, 235
  passed). W-FLOW-2 (b8163e37) redid the work from scratch on
  the same branch and completed it. No work lost beyond the
  unfinished suite run.
- The merged verification chain's first launch failed
  immediately (cargo not on PATH in the nohup shell, exit 127);
  the coordinator's own pkill then matched its own command line
  and killed its own shell before the restart (same self-kill
  class as the Oct 21 incident). Relaunched with an explicit
  cargo PATH; the chain is running.
- One exec in W-FLOW-2's session recorded exit 2; the
  coordinator could not identify the command from the session
  files. All substantive W-FLOW-2 claims were verified
  independently from the primary logs, so it has no material
  impact; recorded here for the critic.

## Next blockers, in order

1. Emit FFMUX selections in the flow FASM (BYP vs D6 decision);
   without it no flow bitstream verifies under US-09.
2. Port the N7 band check into bitstream_decode.rs (decode exit
   1 on band-only streams).
3. Resolve the FF INIT polarity flag (INTERFACES.md item 5) with
   a Vivado-known-INIT experiment.
4. Wire set_int_columns for INT_X2+ on xczu3eg.
5. IOB placement (2 nets skipped per run).
6. Counter ROUTE OveruseTimeout congestion.
7. No documented xczu3eg timing.
