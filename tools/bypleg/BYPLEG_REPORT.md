# BYP-leg flow lane report

Lane branch: fm/ultra-bitgen-bypleg, code tip a899076e (merge of W-IMPL 52c0b265 on sliceout tip 03578179). The lane tip is the report commit itself (this file's commit on the lane branch). No merges into fm/ultra-bitstream (still 07d0d7d), fm/zedboard-merge, or master. No critic dispatched by this lane; Session 37 is the main agent's to dispatch.

## 1. Outcome

A BYP-leg flow design now flows end to end and verifies. The design byp2.sv (two toggle registers) emits FFMUXA1.D6 and FFMUXA2.BYP from a real `helion impl` run on xczu3eg, and its US-09 verify-bit PASSES on fixture RTL: decode exit 0, route 17 PIPs / 6 nets with no violations, extract ok, cosim 800 comparisons, 0 mismatches. Raw RTL fails on cosim only (800 RTL unknowns, the known pre-existing uninitialized-register artifact).

The lane's acceptance is met. The flow already emitted BYP before this lane; the work was a checker gap, not a flow gap: the sliceout `_slice_output_pins` fix counted a LUT O pin as a driver only when the SAME letter had an FFMUX/OUTMUX feature, and byp2 drives FF A2 from LUT B, a cross-letter leg. This lane generalized that rule. No emitter change, no Rust touched.

## 2. The BYP decision condition (baseline)

In `ffmux_selections`, crates/helion-cli/src/ultra.rs:884-940: a FF takes D6 if and only if its D net is driven by a LUT O packed into the same slice AND with the same BEL letter (LUT letter = BEL 0..7 to A..H; FF letter = BEL/2, lines 916-921). BYP otherwise: same-slice different-letter LUT, other-slice LUT, FF Q, carry output, port, or no D net (doc comment lines 871-879, fallback line 937). Emission is `emit_ffmux_fasm` in crates/helion-ultrascale/src/fasm_gen.rs:195-218, `FfMuxSel` at :159-168, called from ultra.rs:625-632. The router drops same-slice same-letter LUT-O to FF-D sinks from routable nets (ultra.rs:1053-1090); all other LUT-O to FF-D edges route to the AX pin. Corpus basis in tools/ffmux/FFMUX_REPORT.md section 2.

## 3. Baseline (W-BASELINE, unmodified base 03578179)

Suites, recounted from logs: helion-ultrascale 432 passed / 0 failed / 1 ignored; helion-cli 72/0/1 with the tileconn env unset; US-09 checkers 88 tests OK.

Flow blinky on xczu3eg with the tileconn map: exit 0, FASM 12 lines with FFMUXA1.D6, 4 PIPs, .bit 5,982,532 bytes, md5 d82b70c8e29ce72f28e08c94f3a16d16 (byte-identical to sliceout), WNS_PS=9650, verify-bit PASS on fixture RTL. Route reads 4 PIPs / 4 nets here (sliceout recorded 3 nets for the identical bit; the 4th net is the A_O net now counted by the sliceout fix).

BYP-candidate design byp2.sv (~/workspace/bypleg-logs/baseline/designs/byp2.sv):

```systemverilog
module byp2 (
    input  logic clk,
    output logic led1,
    output logic led2
);
    logic q1;
    logic q2;
    always_ff @(posedge clk) begin
        q1 <= ~q1;
        q2 <= ~q2;
    end
    assign led1 = q1;
    assign led2 = q2;
endmodule
```

The packer puts both LUTs and both FFs in one slice with no driver affinity, so FF q1 (letter A) is driven by LUT A = D6, and FF q2 (letter A) is driven by LUT B = BYP. Flow exit 0, FASM has SLICE_X1Y0.FFMUXA1.D6 and SLICE_X1Y0.FFMUXA2.BYP, 17 PIPs (4 q1 feedback, 9 q2 feedback, 4 BYP leg B_O to AX), .bit md5 ef0a6ea1304adf4eeb70185702396bc9. Verify-bit FAILED overall on route only: decode, parse, ports, place, extract all ok; cosim 800 comparisons, 0 mismatches. Exact failure line: `undriven sink: PIP INT_NODE_IMUX_40_INT_OUT1.LOGIC_OUTS_W12 source on INT_X1Y0 on net {INT_X1Y0.LOGIC_OUTS_W12, CLEM_X1Y0.CLE_CLE_M_SITE_0_B_O}` plus four `driven only by a PIP` nets on the B_O to AX chain.

Root cause pinned by the baseline: `_slice_output_pins` in tools/us09v/us09v/routecheck.py:278-303 adds `{c}_O` as a driver only when the same letter `c` has an FFMUX/OUTMUX feature. The sliceout BYP repro was same-letter (LUT A to FFMUXA1.BYP), so its fix did not cover this cross-letter case.

## 4. Implementation (W-IMPL, commit 52c0b265)

Commit 52c0b265 "bypleg: count cross-letter BYP LUT O as a route driver", 4 files, +153/-1, all under tools/us09v (routecheck.py +11, test_routecheck.py +75, test_extract.py +28, tests/__init__.py +40). Merged into the lane branch as a899076e.

Rule change: in `_slice_output_pins`, `{c}_O` now counts as a route driver when the LUT is live (INIT present) and any FFMUX in the slice selects BYP, whatever its letter, in addition to the existing same-letter FFMUX/OUTMUX rule. This matches the flow's own semantics (D6 only for same slice same letter, BYP otherwise). A D6 on another letter still does not count, OUTMUX stays same-letter, and the liveness condition is unchanged. No extract change was needed: the sliceout alias already declares `{c}_O` per live LUT letter, so B_O resolves, pinned by a test.

Proof the fix is necessary: stash-and-rerun at both levels. With only routecheck.py stashed, the byp2 verify-bit FAILs on route exactly as the baseline did (17 PIPs, 5 nets, undriven LOGIC_OUTS_W12 / B_O sink plus four driven-only-by-PIP nets), and the new cross-letter unit test fails the same way; with the fix restored, both PASS. Negative tests confirm the check is not weakened: no-INIT B_O is not a driver, no-BYP-selection (D6 only) B_O is not a driver, and a driverless PIP chain still fails.

Results on the impl branch, recounted from logs: US-09 checkers 93 tests OK (88 baseline + 5 new: 4 cross-letter routecheck tests plus 1 B_O extract pin test); helion-ultrascale 432/0/1; helion-cli 72/0/1 (env unset). No Rust test moved. byp2 verify-bit with fixture RTL: PASS (decode 96 FASM lines exit 0, route 17 PIPs / 6 nets, extract ok with 18 warnings, cosim 800/0). Raw RTL: FAIL on cosim only, 0 comparisons, 800 RTL unknowns. Flow blinky still PASS (route 4 PIPs / 4 nets, cosim 400/0). Both .bit files byte-identical to baseline.

## 5. Validation (W-VALIDATE, independent re-runs off a899076e)

All 5 claims reproduced from the validator's own branch:

1. Suites: ultrascale 432/0/1, CLI 72/0/1 (env unset), checkers 93 OK. All exit 0, recounted from the validator's logs.
2. byp2 flow end to end: exit 0, FASM has FFMUXA1.D6 and FFMUXA2.BYP, 17 PIPs. Verify-bit with fixture RTL: PASS on both .bit variants (route 17 PIPs / 6 nets, cosim 800/0). Raw RTL: FAIL on cosim only, 800 RTL unknowns, exactly as claimed.
3. Both sides of the fix: base (03578179 checker + baseline decoder) FAILs on route only, 17 PIPs / 5 nets, with the exact undriven B_O sink line; the validator's base verify log is byte-identical to the baseline log. Merged branch: PASS.
4. Flow blinky: .bit md5 d82b70c8e29ce72f28e08c94f3a16d16, verify-bit PASS, route 4 PIPs / 4 nets, cosim 400/0. Flow log byte-identical to the impl run.
5. Negative checks: all 5 new tests pass individually; the 3 negatives assert the specific LOGIC_OUTS_W12 undriven-sink and driven-only-by-PIP violations. The 2 pre-existing same-letter negatives also pass individually.

Finding: the byp2 router is nondeterministic across processes for one leg. The validator's first flow run produced .bit md5 ac2b92e4487cde3710935549fcc9a3f0, not the claimed ef0a6ea1. The FASM differs in exactly 2 lines: the router picked INT_NODE_SDQ_79_INT_OUT1 where the impl run used INT_NODE_SDQ_82_INT_OUT0 (same source LOGIC_OUTS_W23, same sink, same 17-PIP count). A rerun from the same binary reproduced ef0a6ea1 byte-identically, so the md5 is not a stable expectation for this design. The stable facts (17 PIPs, FFMUX lines, verify-bit PASS) reproduce on both variants. Future lane acceptance should pin verify outcomes, not .bit md5, for multi-leg routes. This is the same nondeterminism class as the counter OveruseTimeout net varying run to run.

Second note: verify-bit needs an absolute --workdir. The relative form printed in the baseline/impl reports fails at cosim because cosim.py runs vvp with cwd=workdir on a relative path. Future lane reports should print the absolute invocation.

## 6. Incidents, all recorded

- Execution-risk notices fired three times (lane worktree add at setup, a worktree listing in impl, harness-only work in validate). Per the N7 CTO precedent (harness-only, own worktree, no merges) none applied; each is recorded in its workstream report and work continued.
- Two cargo runs were SIGTERM-killed mid-compile by runtime restarts (baseline ultrascale after about 12 min, impl first run). Both were relaunched from scratch and completed.
- iverilog was missing from the VM twice (baseline first checkers run: 74 pass / 14 errors, all FileNotFoundError: iverilog; validate run). Installed Icarus Verilog 12.0 via apt in both cases; checkers reran to 88 and 93 OK respectively.
- Nit N34 (Critic Session 36): verify.py defaults US09V_DECODER_BIN to a stale pre-N7 decoder binary that fails decode with exactly 41 band bits. Every verify-bit PASS in this lane used a decoder built from the lane branch via the env override. The default is still not fixed; it is a harness default defect, not a lane defect.
- One exec in the validate session returned exit 2: the relative --workdir verify-bit invocation noted above. Explained, not a lane defect.

## 7. Next blockers, in order

1. Counter ROUTE OveruseTimeout (net varies run to run; always tile (1,0), 50 iters; acceptance should pin the failure class, not the net).
2. IOB placement (4 nets skipped on byp2, 2 on blinky per flow run).
3. `set_int_columns` for INT_X2+ (unproven; byp2 routes through INT_X1 only).
4. The missing known-INIT Vivado datum (P-02 blocked on the missing xcku3p DB).
5. Documented xczu3eg timing.
6. The remaining 54,907 non-band unexplained corpus bits.
7. The byp2 router nondeterminism (one leg varies across processes) is a reproducibility gap for future acceptance, not a correctness gap: both variants verify PASS.

Flags A-CTRL and A-F7F8 still unresolved, need a CTO ruling; this lane touches neither and closes neither. N16 parse-side stays open. N9 block types 2+ untested. N34 (stale default decoder in verify.py) stays open.

## Evidence

- Lane report: ~/workspace/ultra-bitgen-crew/BYPLEG_REPORT.md (this file; copy committed at tools/bypleg/BYPLEG_REPORT.md on the lane branch).
- Logs: ~/workspace/bypleg-logs/baseline/, ~/workspace/bypleg-logs/impl/, ~/workspace/bypleg-logs/validate/.
- Branches: lane fm/ultra-bitgen-bypleg at a899076e (code tip; lane tip is the report commit on top); impl fm/ultra-bitgen-bypleg-impl at 52c0b265; baseline fm/ultra-bitgen-bypleg-baseline at 03578179 (no commits); validate fm/ultra-bitgen-bypleg-validate at a899076e (no commits).
- Design: ~/workspace/bypleg-logs/baseline/designs/byp2.sv (also printed in full in section 3).
