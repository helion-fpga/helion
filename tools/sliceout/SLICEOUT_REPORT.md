# _slice_output_pins lane report

Lane branch: fm/ultra-bitgen-sliceout. Code tip 60f355fd (merge of W-IMPL 1c883b53 onto N7-port base 0bac56f6). The lane tip is the report commit itself, which adds this file; the COORD.md entry names the exact tip hash.

Base: fm/ultra-bitgen-n7-port at 0bac56f6 (Critic Session 35 PASS). No merges into fm/ultra-bitstream (still 07d0d7d), fm/zedboard-merge (still 7ec3192), or master. No critic dispatched by this lane; the main agent dispatches Session 36.

## 1. The defect

`tools/us09v/us09v/routecheck.py` lines 267-289, function `_slice_output_pins`: the LUT O pin `{c}_O` (e.g. `A_O`) was NEVER added as a driver. A slice Q pin counted only when an FFMUX feature selected it; `{c}MUX` only via OUTMUX. So a LUT driving the fabric through its `A_O` site pin was not counted as a driver, and the net it drives failed route as undriven / driven-only-by-a-PIP. The D6 blinky route passed, but any FASM with AX-leg PIPs (the BYP shape: LUT-O to FF-D through the INT to the AX site pin) failed route on `A_O`. This was named blocker 1 in N7_PORT_REPORT.md section 10 and specified in tools/ffmux-scope/SCOPE.md section c items 4-5.

## 2. The fix (commit 1c883b53, 5 files, +164/-2, all in tools/us09v)

- `routecheck.py` (+12): `_slice_output_pins` now adds `{c}_O` as a driver when the LUT is live (`init_value` is not None) and the letter is selected by an `FFMUX{c}{1,2}.` or `OUTMUX{c}.` feature. Q/Q2 and MUX behavior unchanged.
- `extract.py` (+6): `_model_slice` declares one alias line per live LUT letter next to the O6/O5 declarations: `wire s_<site>__{c}_O = s_<site>__{c};`. The alias was proven needed, not added blindly: with the routecheck fix only, the BYP verify passed route but still failed cosim at elaboration on the undefined `s_SLICE_X1Y0__A_O`.
- Tests (5 new): `TestSliceOutputPin` in `test_routecheck.py` (BYP shape passes, OUTMUX selection also counts the O pin, no-INIT and no-selection shapes still fail route on the undriven AX leg) plus an extract test pinning the alias wire and its iverilog compile. A shared `byp_shape_tiles()` helper lives in `tests/__init__.py`. Both fixes were proven necessary by stash-and-rerun: without the routecheck fix the 2 positive tests fail with the exact A_O violations; without the extract fix the extract test fails.

No emitter change. No Rust touched. `FFMUXA1.BYP` already resolves through the generic lookup (bitgen.rs:1795).

## 3. Baselines (W-BASELINE, unmodified base 0bac56f6)

- Ultrascale suite: 432 passed / 0 failed / 1 ignored (ignored is the N16 mixed-column probe), exit 0.
- CLI suite, tileconn env unset: 72 passed / 0 failed / 1 ignored, exit 0.
- US-09 checkers: 83 tests OK in 141s, iverilog ~/workspace/oss-cad-suite/bin/iverilog (Icarus 14.0 devel).
- All 5 fixtures verify PASS with 0 co-sim mismatches. Golden blinky verify-fasm FAILs on route/extract as expected (undriven sinks, no RTL given).
- Flow blinky: exit 0, .bit 5,982,532 bytes, md5 d82b70c8e29ce72f28e08c94f3a16d16, 12-line FASM with FFMUXA1.D6, 4 PIPs, WNS_PS=9650. Verify-bit with fixture RTL: PASS (decode exit 0, 81 lines; route 4 PIPs, 3 nets; cosim 400/0). Raw RTL: FAIL on cosim only, 400 RTL unknowns (pre-existing uninitialized `logic q`).
- Defect REPRODUCED on the base: the BYP-shape FASM (flow FASM with D6 swapped to BYP plus the two AX-leg PIPs `INODE_W_1_FT1.LOGIC_OUTS_W0` and `BOUNCE_W_0_FT1.INODE_W_1_FT1`) fails verify-fasm with route FAIL on A_O (undriven sink PIP `INODE_W_1_FT1.LOGIC_OUTS_W0`, two nets driven only by a PIP) and cosim failing at elaboration on unbound `s_SLICE_X1Y0__A_O`.
- Report: ~/workspace/sliceout-logs/baseline/BASELINE_REPORT.md. Logs: ~/workspace/sliceout-logs/baseline/.

## 4. Merged verification (W-VALIDATE, independent re-runs on 60f355fd)

- Ultrascale suite: 432 passed / 0 failed / 1 ignored, exit 0 (recounted from the log by the coordinator: 432 passed, 0 failed).
- CLI suite, tileconn env unset: 72 passed / 0 failed / 1 ignored, exit 0 (recounted: 72 passed, 0 failed).
- US-09 checkers: Ran 88 tests in 136.101s, OK (83 baseline + 5 new), iverilog 14.0 oss-cad-suite.
- 5 fixtures all ok; golden blinky test ok (expects FAIL with named undriven-sink reasons). New sliceout tests all ok.
- Flow blinky verify-bit (same .bit, md5 d82b70c8e29ce72f28e08c94f3a16d16): fixture RTL PASS, decode exit 0 (81 lines), route 4 PIPs, 4 nets (up from 3, the live LUT O is now counted), cosim 400/0. Raw RTL: FAIL on cosim only, 400 RTL unknowns.
- BYP-shape verify-bit: independently re-run by W-VALIDATE from its own run_byp.py (same inputs, `--top blinky`, flow-design port map): PASS, route 6 PIPs, 4 nets, cosim 400/0, byte-identical to the impl worker's byp-after.log. On the base code, independently re-run from the baseline worktree at 0bac56f6 (not just the impl's log): FAIL, route undriven A_O sink + 2 PIP-only nets, cosim undefined `s_SLICE_X1Y0__A_O`, differing from the impl's byp-before.log only in workdir path.
- Negative case: `test_no_init_lut_o_is_not_a_driver` and `test_no_selection_lut_o_is_not_a_driver` both pass, so genuinely undriven nets still fail route. The fix does not weaken the check into accepting undriven nets.
- Report: ~/workspace/sliceout-logs/validate/VALIDATE_REPORT.md. Logs: ~/workspace/sliceout-logs/validate/.

## 5. Incidents

- Stale decoder binary default: `verify.py` defaults `US09V_DECODER_BIN` to a binary built from fm/ultra-bitgen-decoder at f35560a5, which predates the N7-port decode change and fails decode with exactly 41 unexplained band bits. All PASS results in this lane used a decoder built from the lane base/branch (`cargo build --release --bin helion-bitgen`, pointed at via US09V_DECODER_BIN). The stale run is preserved as `verify-flow-blinky-fixture-stale-decoder.log` in the baseline logs. Future lanes must build the decoder from their branch or they will measure the wrong decoder. This is a harness default defect, not a lane defect; it is not fixed here.
- One execution-risk notice fired on the lane `git worktree add` result at setup. Per the N7 CTO precedent (harness-only work, own worktree, no merges), the lane continued; recorded here. W-IMPL and W-VALIDATE reported no notices. W-BASELINE reported none requiring a stop.
- A stray `helion impl --help` in the baseline worktree ran the default HL10T flow in the worktree root; it wrote no artifacts and the worktree is clean.
- W-VALIDATE guessed the wrong test class name once (`TestExtract` vs the real `TestVerilogOutput`); the corrected run is OK, no lane defect.

## 6. Carried

- The BYP-leg flow design end to end is still unexercised: BYP emission is implemented and unit-tested in the flow, and the checker now accepts the BYP shape, but no flow run has emitted a BYP-leg design. Next blocker.
- Counter ROUTE OveruseTimeout, same class as the GAP-F5 base (net varies run to run; always tile (1,0), 50 iters).
- IOB placement (2 nets skipped per flow run); set_int_columns for INT_X2+; known-INIT Vivado datum; documented xczu3eg timing; remaining 54,907 non-band unexplained corpus bits.
- N16 parse-side stays open (ignored probe untouched). N9 block types 2+ untested. Flags A-CTRL and A-F7F8 still unresolved, need a CTO ruling; this lane touches neither and closes neither.

## 7. Next blockers, in order

1. A BYP-leg flow design end to end.
2. Counter ROUTE OveruseTimeout.
3. IOB placement.
4. `set_int_columns` for INT_X2+.
5. The missing known-INIT Vivado datum (P-02 blocked on the missing xcku3p DB).
6. Documented xczu3eg timing.
7. The remaining 54,907 non-band unexplained bits across the corpus.

## Evidence

- Lane report: ~/workspace/ultra-bitgen-crew/SLICEOUT_REPORT.md (this file; copy committed at tools/sliceout/SLICEOUT_REPORT.md on the lane branch).
- Logs: ~/workspace/sliceout-logs/baseline/, ~/workspace/sliceout-logs/impl/, ~/workspace/sliceout-logs/validate/.
- Branches: lane fm/ultra-bitgen-sliceout at 60f355fd (code tip); impl fm/ultra-bitgen-sliceout-impl at 1c883b53; baseline fm/ultra-bitgen-sliceout-baseline at 0bac56f6 (no commits); validate fm/ultra-bitgen-sliceout-validate at 60f355fd (no commits).
