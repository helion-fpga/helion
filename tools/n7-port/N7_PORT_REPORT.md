# N7-port lane report: N7 band check ported into bitstream_decode.rs

Lane branch: `fm/ultra-bitgen-n7-port` at `9ebbf159` on origin (code tip `f9f0962a` merged as `9ebbf159` onto FFMUX base `83c1440e`).
Report: `~/workspace/ultra-bitgen-crew/N7_PORT_REPORT.md` (copy committed at `tools/n7-port/N7_PORT_REPORT.md` on the lane branch).
Date: Nov 22, 2026. No em dashes in this file.

## 1. Headline

Decode now applies the N7 band check. A freshly flow-emitted blinky `.bit` on xczu3eg decodes with exit 0 and 0 unexplained bits (was exit 1 with exactly 41 H-1 band bits), and **verify-bit with fixture RTL now prints `blinky: PASS` overall**: decode ok, parse ok, ports ok, place ok, route ok, extract ok, cosim 400 comparisons with 0 mismatches. This is the first fully green verify-bit on a flow-emitted design.

The 14-pair corpus decode total falls from 2,722,823 unexplained bits to **54,907**, all non-band; band-range (1440-1487) unexplained is 0 on every pair. All 14 decoded FASM outputs are byte-identical to baseline, so no feature regressions.

Lane acceptance is met on all four points: blinky decode exit 0 with 0 unexplained, corpus band portion resolved per the N7 rule with no non-band bit newly hidden, suites green at baseline plus new tests, honest arithmetic throughout.

## 2. What changed

Commit `f9f0962a` ("Port N7 band check into bitstream decode"), 6 files, +506/-113, merged `--no-ff` as `9ebbf159`:

- `crates/helion-ultrascale/src/bitstream_decode.rs` (+195/-?): `decode_frames` now derives per-frame LUTRAM masks from decoded features using the same identifiers as the parse side (`ram_mode_lut_index`, `lut_init_index`, `record_lutram_bit`, now `pub(crate)` in `bitgen.rs` so the mask rule cannot drift), then runs `check_stored_band` per frame when building `unexplained`. Band bits 1440-1487 of a frame whose check is Exact or QResidual are explained, not listed. A Mismatch frame fails loud with a `BandMismatch` error naming the FAR, stored and expected band. Block-2+ frames are not checked; their band bits stay unexplained (N9 carried).
- `crates/helion-ultrascale/src/bitgen.rs`: the three mask identifiers made `pub(crate)`; no behavior change.
- `crates/helion-ultrascale/tests/bitstream_decode.rs` and `tests/n7_decode_mixed_column.rs` (new, 3 tests): new decode tests, including the mixed-column cases.
- `crates/helion-cli/src/ultra.rs` and `crates/helion-ultrascale/src/bin/helion-bitgen.rs`: test-expectation updates only. The blinky flow test and the emitter band test now assert the unexplained set is empty instead of exactly the 41 band bits; doc comments updated to match. No decoder or emitter behavior changed in these files.

Mask derivation mirrors the parse side's ordering trap: INIT precedes MODE.RAM in feature order, so candidate INIT ref positions are collected during the window loop and filtered by the RAM set after the loop. Only RAM-mode LUT INIT positions are masked; logic-mode INIT bits are never masked.

## 3. Suites

Merged branch (W-VALIDATE recounts from the logs):

- Ultrascale: **432 passed / 0 failed / 1 ignored** (426 baseline + exactly 6 new tests). The 1 ignored is the N7 parse-side probe test (`mixed_column_ram_plus_logic_false_rejects`, known limitation, expected).
- CLI, tileconn env unset: **72 passed / 0 failed / 1 ignored**.
- US-09 checkers: **83 tests OK** (103.5s; iverilog from `~/workspace/oss-cad-suite/bin` on PATH).
- Release build exit 0.

## 4. Baselines (W-BASELINE, on unmodified base 83c1440e)

- Ultrascale 426/0/1, CLI 72/0/1, checkers 83 OK.
- Fresh blinky flow + decode: flow exit 0, `.bit` 5,982,532 bytes, md5 `d82b70c8e29ce72f28e08c94f3a16d16`; decode exit 1, `decode failed: 41 set bits no feature explains`, all absolute bits 1440..1486.
- Corpus decode total **2,722,823** unexplained; raw band-range count 2,667,916; gap band 2,646,705 (raw band minus 21,211 claimed-column band lines). Per-pair counts match the H1_BASELINE table exactly.

## 5. Corpus decode re-run (merged branch)

Per-pair unexplained (strict `^unexplained: FAR` count): P-11 2,166; P-13 5,673; P-14 6,174; P-15 6,519; P-16 3,987; P-17 4,075; P-23 2,528; P-28 1,830; P-40 2,658; P-41 2,620; P-42 576; P-43 1,341; P-44 1,458; P-45 13,302. **Total 54,907, all non-band; band-range unexplained = 0 on every pair.** All 14 decodes exit 1 (unexplained non-band bits remain); no `BandMismatch` in any log.

Arithmetic, stated honestly: the task brief expected 76,118 (= 2,722,823 - 2,646,705 gap band). The actual total is 21,211 lower, and 21,211 is exactly the baseline claimed-column band-lines sum (442 + 718 + 587 + 19,464). The port explained the **full** band range (2,667,916 bits), not only the gap band: every per-pair delta equals that pair's baseline band count exactly. The baseline subtraction of the 21,211 claimed-column lines was conservative; the decode run shows those bits are explained by the same N7 check, because the check runs per frame on the stored band, and a band bit claimed by a segbits feature is still a band bit the check accounts for. Nothing supports the claimed-column bits staying unexplained: band-range unexplained is 0, FASM unchanged, band_check exact.

Band accounting verified independently (W-VALIDATE rebuilt and ran `band_check` from the merged branch over the corpus frames/masks): block-0 frames 144,816, MAP-A exact **144,332**, residual w **484**, residual other **0**. Per-pair lines identical to `~/workspace/n7-band-check.log` and N7_MASK_REPORT.md. Other = 0 means no Mismatch anywhere.

Logs: `~/workspace/n7-port-logs/validate/corpus-decode/` (`counts.txt`, per-pair `.fasm`/`.log`), `~/workspace/n7-port-logs/validate/band-check-corpus.log`.

## 6. Blinky flow + verify-bit (merged branch)

- Flow exit 0; emitted `.bit` byte-identical to the baseline flow-run bit (md5 `d82b70c8e29ce72f28e08c94f3a16d16`, 5,982,532 bytes, 4 PIPs, WNS_PS=9650).
- Decode of the emitted bit: **exit 0, 0 unexplained** (baseline: exit 1, exactly 41 H-1 band bits). Decoded FASM identical to baseline (81 lines).
- **Verify-bit, fixture RTL: `blinky: PASS`.** decode ok (81 lines, exit 0); parse ok (2 tiles, 81 features); ports ok; place ok; route ok (4 PIPs, 3 nets); extract ok (1 site, 18 warnings); cosim ok (400 comparisons, 0 mismatches, 0 unknowns).
- **Verify-bit, raw RTL: FAIL overall on cosim only**, exactly as expected: decode/parse/ports/place/route/extract all ok; cosim 0 comparisons, 0 mismatches, **400 RTL unknowns** from the pre-existing uninitialized `logic q` in `examples/blinky.sv`.
- Logs: `~/workspace/n7-port-logs/validate/flow-run/` (`flow-blinky.log`, `verify-blinky-fixture.log`, `verify-blinky.log`).

## 7. Counter stretch

OveruseTimeout unchanged in class; the failing net varies run to run. First run: `route failed for net 'cnt_0_d' at wire 34 at tile (1,0) shared by 2 nets after 50 iters: OveruseTimeout`. Eight runs with the same binary and inputs: cnt_1/wire 36/3 nets 4 times, cnt_0_d/wire 34/2 nets 3 times, cnt_2/wire 36/2 nets once; always tile (1,0), always 50 iters, always OveruseTimeout. This is pre-existing router nondeterminism (net order varies per process), not a lane change: the lane diff is decode/test-only, and no routing code is touched. Future acceptance text for the counter should pin the failure class (OveruseTimeout, tile (1,0), 50 iters) rather than a specific net/wire.

## 8. N16 does not reproduce in decode

Key empirical finding (W-IMPL probe, confirmed stronger by W-VALIDATE): MODE.RAM bits in real Vivado bitstreams are per-slice (offset-shifted), not shared. W-IMPL probe over P-11/P-13/P-40: 4,992 cases, zero shared-only. W-VALIDATE re-ran the same script across all 14 pairs: **26,556 cases checked, 0 shared-only** (per-slice-only 22,052, both 4,504, neither 0). The 4,992 figure in the merge commit message is the 3-pair subset, not an error; the 26,556 figure is the all-14 scope. The conclusion holds either way.

Consequence: the decoder's per-tile-window offset model detects MODE.RAM correctly on every slice, so the mixed-column false-reject does not occur in decode. The mixed-column decode tests pass (3/3, 0 ignored): `logic_only_column_decodes_clean`, `mixed_column_ram_plus_logic_decodes_clean`, `mixed_column_with_unmasked_band_fails_loud`. The parse-side N16 ignored probe is untouched and still ignored; parse-side N16 stays open.

Side note: Helion's emitter still writes MODE.RAM at shared Y0 positions (parse-model encoding), which differs from Vivado's per-slice encoding; pre-existing emitter matter, out of this lane.

## 9. Incidents and deviations, all recorded

- W-IMPL deviations: mixed-column pin test passes instead of ignored (finding above); CLI suite must run without the tileconn env for the 72/0/1 count (with-env run fails the two `*_stops_at_route_without_pin_wire_map` tests because the flow routes past that point; env artifact, not a regression; the with-env run is what proved the fresh blinky flow test passes); stale `/tmp/helion-us09-flow-blinky/blinky.xcku3p.bit`/`.fasm` from an earlier with-env run failed one CLI test until deleted (pre-existing test-isolation flaw in the shared temp path, not caused by this change); rustfmt fixed only the worker's own lines, pre-existing diffs left untouched.
- W-BASELINE incidents: first CLI run killed mid-compile by a runtime restart (rerun clean); checkers suite needed `~/workspace/oss-cad-suite/bin` on PATH for iverilog (first attempt: 13 `FileNotFoundError: 'iverilog'` errors; preserved as `us09-checkers-no-iverilog-on-path.log`).
- No corpus re-run or 100-design loop by W-IMPL, per its brief (validation worker's scope; no emitter change).

## 10. Next blockers, in order

1. Route checker `_slice_output_pins` fix (LUT O not counted as a driver; D6 blinky route passes, but FASMs with AX-leg PIPs still fail route on A_O).
2. A BYP-leg flow design end to end (BYP emission implemented and unit-tested but unexercised in the flow; blinky exercises D6 only).
3. Counter ROUTE OveruseTimeout (plus the nondeterminism note in section 7).
4. IOB placement.
5. `set_int_columns` for INT_X2+.
6. The missing known-INIT Vivado datum (P-02 blocked on the missing xcku3p DB).
7. Documented xczu3eg timing.
8. The remaining 54,907 non-band unexplained bits across the corpus (H-2 to H-10 and the rest of the hole list).

Carried: N9 (block types 2+ untested, band stays unexplained there); N16 parse-side (mixed-column false-reject on parse, ignored probe still in place); N1-N6, N8, N10-N15, N17, N18-N21, N22-N28, N29-N31. Flags A-CTRL and A-F7F8 still unresolved, need a CTO ruling.

No critic dispatched by this lane; the main agent dispatches Session 35. No merges into `fm/ultra-bitstream` (still `07d0d7d`), `fm/zedboard-merge` (still `7ec3192`), or master.

## Evidence

- Lane report: `~/workspace/ultra-bitgen-crew/N7_PORT_REPORT.md` (this file; copy at `tools/n7-port/N7_PORT_REPORT.md` on the lane branch).
- Logs: `~/workspace/n7-port-logs/baseline/`, `~/workspace/n7-port-logs/impl/`, `~/workspace/n7-port-logs/validate/`.
- Branches: lane `fm/ultra-bitgen-n7-port` at `9ebbf159`; impl `fm/ultra-bitgen-n7-port-impl` at `f9f0962a`; baseline `fm/ultra-bitgen-n7-port-baseline` at `83c1440e` (no commits); validate `fm/ultra-bitgen-n7-port-validate` at `9ebbf159` (no commits).
