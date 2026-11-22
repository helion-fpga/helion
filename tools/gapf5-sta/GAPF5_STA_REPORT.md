# GAP-F5 STA lane report: preliminary xczu3eg timing DB

Date: 2026-11-21, about 22:45 EST. Lane branch: fm/ultra-bitgen-gapf5-sta at bf6adddebe00650517399ee8262ed3fd760f00be (merge of W-IMPL 0da3932e onto base 60523a00, the US-01 tip). Workers: W-IMPL (06373baa) and W-BASELINE (d02c7ea1), both complete. Coordinator: 59dbfd25 (merge, push, merged verification launch); tail coordinator c2ee7b96 (verification recount, flow outcomes, this report).

## Headline

GAP-F5 is closed at STA: `helion impl` on xczu3eg no longer stops at STA on any of the four runs. Every run now prints the timing line (WNS_PS=9650, estimate=1) and reaches ROUTE. No design flows end to end yet and no bitstream was emitted: the furthest run, blinky with the tileconn map, now stops at BITGEN because xczu3eg has no emitter device. The next blockers are named below, not hidden.

## What changed

One commit, 0da3932e, 3 files, +38/-3, merged --no-ff as bf6addde (clean, no conflicts):

- `crates/helion-ultrascale/src/timing_db.rs`: new `preliminary_xczu3eg()`, an exact clone-and-rename of `preliminary_xcvu3p()` with only the part name changed. Same generic UltraScale+ AMD forum ballpark (120 ps LUT + 180 ps one INT hop = 300 ps per logic level), every value Ballpark, "Do not use for signoff." Plus a `"xczu3eg"` arm in `sta_db_for_part`. Unknown parts still fail loud.
- `crates/helion-ultrascale/src/lib.rs`: export the new function.
- `crates/helion-ultrascale/tests/timing_db.rs`: two new tests pinning the preliminary values and the differ-only-by-part property, plus `sta_db_for_part("xczu3eg")` selection asserts (part is xczu3eg, `is_estimate()` true).

No `documented_xczu3eg()` was added: the DS925 Block RAM table with a TRCKO_DO value for xczu3eg was not retrievable from public sources, and no delay was invented or transplanted from a sibling datasheet. That stays an open, named gap, not a silent omission.

## DB derivation, stated plainly

There is no real xczu3eg timing data in this lane. The DB reuses the same generic UltraScale+ ballpark that `preliminary_xcku3p()` already reuses from `preliminary_xcvu3p()`, under the xczu3eg part name, labeled preliminary estimate everywhere (code doc comment, `is_estimate()`, and the flow's own `estimate=1` flag on the timing line). Every timing number the flow prints on xczu3eg is an estimate. Tier 2 still waits on Vivado; nothing here is Vivado-correlated and no Vivado equivalence is claimed.

## Suites, merged branch (recounted by the tail coordinator from the logs)

- helion-ultrascale: 423 passed / 0 failed / 1 ignored across 31 test-result lines, 0 FAILED lines, ULTRA_EXIT:0. Baseline on 60523a00 was 421/0/1, so the delta is exactly the 2 new timing_db tests.
- helion-cli: 66 passed / 0 failed / 1 ignored across 11 result lines, 0 FAILED lines, CLI_EXIT:0. Baseline 66/0/1, unchanged.
- Release build: BUILD_EXIT:0.

Logs: ~/workspace/gapf5-logs/test-ultrascale-merged.log, test-cli-merged.log, build-cli-merged.log.

## Flow outcomes, merged branch (read in full by the tail coordinator)

All four runs pass synth, pack, place, legality, and STA. Baseline for comparison: on 60523a00 all four stopped loud at STA with `impl: ULTRA STA: timing: unsupported part xczu3eg`, exit 1.

1. Blinky on xczu3eg WITH the tileconn map (HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp, HELION_TILECONN_PART=xczu3eg-sbva484-1-e). Exit 1. Furthest xczu3eg flow ever: synth 1 LUT 1 FF, pack 1 slice, place, legality 0 violations, timing WNS_PS=9650 TNS_PS=0 endpoints=1 r2r_ps=270 estimate=1, wiredb 1475 wires tileconn entries=81640 pairs=658732, route nets=2 iters=1 overused=0, route-to-pip pips=6, fasm lut_lines=1. Then fails loud at BITGEN: `impl: ULTRA BITGEN: part xczu3eg has no emitter device yet: only xcku3p is wired; xcvu3p.json is provisional until its frame-total gap is resolved`. Log: flow-blinky-xczu3eg-tileconn-merged.log.
2. Blinky on xczu3eg WITHOUT the tileconn map. Exit 1. STA passes (same timing line). ROUTE fails loud: `impl: ULTRA ROUTE: route failed for net 'q' at driver pin 1.AQ.Q has no wire: NoPath`. Same NoPath as the US-09 xcku3p behavior, as expected without the pin map. Log: flow-blinky-xczu3eg-merged.log.
3. Counter on xczu3eg WITH the tileconn map. Exit 1. STA passes (WNS_PS=9650, endpoints=4). ROUTE fails loud with a new failure mode, not NoPath: `impl: ULTRA ROUTE: route failed for net 'cnt_1' at wire 36 at tile (1,0) shared by 3 nets after 50 iters: OveruseTimeout`. The router reaches a real routing conflict (a shared wire overused by 3 nets) instead of failing to find a path. Log: flow-counter-xczu3eg-tileconn-merged.log.
4. Counter on xczu3eg WITHOUT the tileconn map. Exit 1. STA passes. ROUTE fails loud: `impl: ULTRA ROUTE: route failed for net 'cnt_3' at driver pin 1.AQ.Q has no wire: NoPath`. Log: flow-counter-xczu3eg-merged.log.

Observation recorded, not a blocker for this lane: in every run the router skips two nets with `reason=IOB cell u_iob has no placed site` (blinky: q, led; counter: cnt_3, led). IOB cells have no placed site in the flow; the skipped nets are not routed. In the blinky-with-tileconn run the router still emitted 6 PIPs and reached BITGEN, so the skip did not stop the flow there, but the mechanism is not proven and the skip lines are quoted verbatim above rather than interpreted.

## What GAP-F5 did and did not do

- Did: unblocked STA on xczu3eg with a preliminary, honestly labeled DB, following the exact precedent the xcku3p DB already set. All four flows now reach ROUTE or beyond.
- Did not: emit a bitstream, route counter with the map, wire an xczu3eg emitter, place IOB cells, or produce real xczu3eg timing. None of those is claimed.

## Next blockers, in order

1. xczu3eg emitter device missing at BITGEN (blinky with the map reaches it). Unblocker: an xczu3eg emitter device wired the way xcku3p is, with its frame map.
2. Counter with the map hits ROUTE OveruseTimeout (wire 36, tile (1,0), shared by 3 nets, 50 iters). Unblocker: router congestion handling or placement spreading that resolves the shared-wire overuse; the multi-sink translate limitation from US-09 is a separate, still-open item.
3. Without the tileconn map, ROUTE still fails NoPath on both designs, as before. The map is required, not optional.
4. IOB cells have no placed site; two nets per design are skipped at route. Unblocker: IOB placement in the flow.
5. No documented xczu3eg timing (no DS925 TRCKO_DO sourced). All timing numbers remain estimates until Vivado data lands.

## Evidence

- Lane branch fm/ultra-bitgen-gapf5-sta, tip bf6adddebe00650517399ee8262ed3fd760f00be on origin (verified via git ls-remote by the coordinator; re-verified by the tail coordinator before this report was committed).
- W-IMPL commit 0da3932e (3 files, +38/-3); merge bf6addde.
- Merged verification logs in ~/workspace/gapf5-logs/: test-ultrascale-merged.log (423/0/1), test-cli-merged.log (66/0/1), build-cli-merged.log (BUILD_EXIT:0), flow-*-merged.log (four runs, all FLOW_EXIT:1 with the stage outcomes above).
- Baseline logs in the same directory (test-ultrascale-baseline.log 421/0/1, test-cli-baseline.log 66/0/1, flow-*-baseline.log all STA stop, exit 1). One baseline incident disclosed by W-BASELINE and the coordinator: a runtime restart interrupted the first CLI baseline run's wait; the run had already completed at 66/0/1 and the re-run matched; the partial log is preserved as test-cli-baseline-partial1.log.
- COORD.md lane entry (this report's completion entry follows the merge entry). No critic dispatched by the lane; the main agent dispatches exactly one critic (Session 32) at the end.
