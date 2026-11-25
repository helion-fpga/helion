# Timing verification record, session 18 second replacement (GAP-F5)

Date: 2026-11-19. Branch fm/ultra-bitgen-timing, base 07d0d7d.
Verified state first: `git log` tip 9b463cf0, `git status` clean, exactly
the 3 predecessor commits (8aa5395e, 7a3ff672, 9b463cf0). No work redone.

## Spot checks against cited sources (independent re-verification)

### DS922 / DS923 Table 33, TRCKO_DO 1.02 ns: CORRECTED (session 18 read 1.46 ns)

Source checked: docs.amd.com DS922 Block RAM and FIFO Switching
Characteristics page, and the DS923 equivalent page. The TRCKO_DO row
(Clock CLK to DOUT output, without output register) reads 0.91, 1.02,
1.11, 1.46, 1.53 ns across the columns -3 at 0.90V, -2 at 0.85V,
-1 at 0.85V, -2 at 0.72V, -1 at 0.72V. Session 18 read the fourth
numeric column (1.46 ns) as 0.85V -2; that column is -2 at 0.72V.
The -2 at 0.85V column is the second numeric column, value 1.02 ns.
`documented_xcku3p()` / `documented_xcvu3p()` store 1020 ps
(corrected from 1460 ps by the N64 fix lane, 2026-11-25; the N66
fix lane corrected this record and TIMING_XCKU3P.md the same day).
Cross-checks on the same pages also match TIMING_XCKU3P.md:
TRCKO_DO_REG 0.29 ns at 0.85V -2, FMAX_WF_NC 738 MHz at 0.85V -2,
UltraRAM only FMAX values plus TPW 700 ps (no clock-to-output delay),
DSP only FMAX values (no pin-to-pin delay). (Session 18 quoted the
fourth-column values 0.42 ns, 585 MHz, 800 ps here; those are -2 at
0.72V. Corrected by the N66 fix lane, 2026-11-25.)

Note: the AMD web pages number this table "Table 1"; "Table 33" is the
PDF numbering used by the code and docs. The value and the table are
the same; the citation stands.

### AMD forum ~0.3 ns per logic level: CONFIRMED, with a provenance limit

Source checked: AMD community thread "Delay per logic level on Virtex
Ultrascale+" (adaptivesupport.amd.com). A poster writes: "Xilinx gives
a ball park of about 0.3 ns per level of logic. This is including logic
& route both." The same poster adds: "This is again my derivation based
on what was in a Xilinx document, it was never stated directly." So the
300 ps total is a second-hand forum ballpark, not a Xilinx-published
number, and the forum post never splits LUT from route. The predecessor
documents this honestly in TIMING_XCKU3P.md section 1.

### Every estimate is labeled: CONFIRMED in code

- `UltraScaleTimingDb::is_estimate()` is true when any field is
  Ballpark-sourced; both preliminary DBs are estimates.
- `TimingReport.is_estimate` is set from the DB; `hang_diag_line`
  appends `estimate=1` when set.
- `sta_db_for_part()` selects per part and fails loud on unknown parts.
- `documented_xcku3p()` / `documented_xcvu3p()` carry exactly one value
  (BRAM CLK to DOUT 1020 ps, corrected from 1460 ps; see the TRCKO_DO
  section above) and UNKNOWN elsewhere, so `sta_routed`
  fails loud on them with `timing: LUT6 delay UNKNOWN in TimingDb`.
- No em dashes in the branch diff (`git diff 07d0d7d..HEAD`: 0).

## Findings from verification (fixed in follow-up commits)

### V-1: ballpark source strings overstate provenance (labeling)

The individual ballpark values beyond the LUT/route split (ff_ckq 150,
ff_setup 80, ff_hold 60, carry8 40/100/60, pip 50 ps) are assumed
allocations inside the 0.3 ns level, not forum-stated values. The
`Ballpark` source string on every one of them names only the forum
level, which reads as if the forum supports each value. Fix: reword
the source strings so each says it is an assumed allocation within the
forum 0.3 ns level. The values themselves do not change; they stay
Ballpark and the reports stay labeled estimate.

### V-2: STA dropped Carry4 cells (latent; NOT exercised by counter.sv)

CORRECTION, recorded after the end-to-end runs: the original V-2
wording ("STA cannot run on counter.sv") was wrong. `helion impl`
on the shipped counter.sv reports `carry_chains=0`: the synth path
emits 4 LUTs and 4 FFs, no Carry4 cells, so STA ran fine on counter
with or without this fix. A 32-bit counter probe (/tmp/widecounter.sv)
also reports `carry_chains=0` (110 LUTs), and no caller of
`extract_carry4_chains_from_luts` exists in helion-sv (only the
re-export in helion-ir lib.rs), so today no synth path in this flow
produces Carry4 cells at all.

What V-2 actually is: a latent defect. `sta_on_placed` built drivers
and fanin only for `Lut6` and `Hff` cells, so any future design that
DOES synthesize Carry4 cells (the extractor exists, pack/place and
the STA stack all handle Carry4) would fail STA with
`timing: no driver for net ...` on every carry-driven FF D pin, and
the carry8 DB fields were never consumed by `sta_routed`. Fix (kept):
model Carry4 cells in the STA netlist with the carry8 `s_to_o`
delay, covered by unit tests (carry path hand-computed at 550 ps,
UNKNOWN s_to_o fails loud). The fix is verified at unit level, not
end-to-end: no in-repo design exercises the carry path through
`helion impl` today. When a synth path starts emitting Carry4, rerun
`helion impl` on a carry design to close that loop.

## What is still genuinely unblockable

A real xcku3p delay DB that can run STA (lut6, ff_ckq, ff_setup,
interconnect.single from a published source) does not exist: DS922
publishes no LUT6, flip-flop, CARRY8, or interconnect delay, confirmed
by the spot checks above. Unblocker (unchanged): licensed Vivado
`report_timing` plus `write_sdf` on probe designs, per
TIMING_XCKU3P.md section 5. License state UNKNOWN; no check run.
Until then STA runs on the preliminary ballpark DB and every report is
labeled estimate. ## End-to-end STA runs (this session, real `helion` binary)

Built with `cargo build -j1 -p helion-cli --offline --bin helion`
(Finished dev profile in 2m 50s, 2 pre-existing bin warnings, none in
ultra.rs). All four runs print the `hang_diag timing` line with
`estimate=1` and then fail loud at ROUTE (expected: GAP-F4 owns the
empty WireDb, not this workstream).

- blinky, xcku3p: `luts=1 ffs=1 carry_chains=0`, 1 slice, legality ok,
  `hang_diag timing WNS_PS=9650 TNS_PS=0 endpoints=1 r2r_ps=270
  iob_ps=0 ms=0 estimate=1`. Hand check: 150 ckq + 120 lut + 0 wire
  (same slice) = 270; WNS = 10000 - 80 - 270 = 9650. Correct.
- counter, xcku3p: `luts=4 ffs=4 carry_chains=0`, 1 slice, legality
  ok, `hang_diag timing WNS_PS=9650 TNS_PS=0 endpoints=4 r2r_ps=270
  iob_ps=0 ms=0 estimate=1`. Same per-path arithmetic, 4 endpoints.
  Helion exit code 1 (the ROUTE fail-loud, as designed).
- blinky and counter, xcvu3p: identical timing lines (same
  preliminary DB values, different part name only).
- 32-bit counter probe (/tmp/widecounter.sv, xcku3p): `luts=110
  ffs=32 carry_chains=0`, 14 slices, `WNS_PS=-250 TNS_PS=-380
  endpoints=32 r2r_ps=10170 estimate=1`. STA finds a real (estimated)
  violation on a long LUT ripple path: the estimator is not vacuous.

No delay number was invented to get around this.

