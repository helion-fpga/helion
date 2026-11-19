# Timing verification record, session 18 second replacement (GAP-F5)

Date: 2026-11-19. Branch fm/ultra-bitgen-timing, base 07d0d7d.
Verified state first: `git log` tip 9b463cf0, `git status` clean, exactly
the 3 predecessor commits (8aa5395e, 7a3ff672, 9b463cf0). No work redone.

## Spot checks against cited sources (independent re-verification)

### DS922 / DS923 Table 33, TRCKO_DO 1.46 ns: CONFIRMED

Source checked: docs.amd.com DS922 Block RAM and FIFO Switching
Characteristics page, and the DS923 equivalent page. The TRCKO_DO row
(Clock CLK to DOUT output, without output register) reads 0.91, 1.02,
1.11, 1.46, 1.53 ns across the columns 0.90V -3/-2/-1, 0.85V -2/-1,
0.72V -2/-1. The fourth numeric column is 0.85V -2, value 1.46 ns.
`documented_xcku3p()` / `documented_xcvu3p()` store 1460 ps. Correct.
Cross-checks on the same pages also match TIMING_XCKU3P.md:
TRCKO_DO_REG 0.42 ns at 0.85V -2, FMAX_WF_NC 585 MHz at 0.85V -2,
UltraRAM only FMAX values plus TPW 800 ps (no clock-to-output delay),
DSP only FMAX values (no pin-to-pin delay).

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
  (BRAM CLK to DOUT 1460 ps) and UNKNOWN elsewhere, so `sta_routed`
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

### V-2: STA cannot run on counter.sv (carry chains dropped)

`sta_on_placed` in `crates/helion-cli/src/ultra.rs` builds drivers and
fanin only for `Lut6` and `Hff` cells. A counter (`cnt <= cnt + 1`)
synthesizes to `Carry4` cells whose `O{lane}` sum outputs drive the FF
D pins, so those D nets have no driver and `sta_routed` errors with
`timing: no driver for net ...`. STA therefore runs on blinky (one LUT
plus one FF) but fails on counter before ROUTE is even reached. The
carry8 DB fields are also never consumed by `sta_routed`. Fix: model
Carry4 cells in the STA netlist with the carry8 `s_to_o` delay, with
regression tests on a carry-chain netlist and an end-to-end `helion
impl` run on blinky and counter through the STA stage.

## What is still genuinely unblockable

A real xcku3p delay DB that can run STA (lut6, ff_ckq, ff_setup,
interconnect.single from a published source) does not exist: DS922
publishes no LUT6, flip-flop, CARRY8, or interconnect delay, confirmed
by the spot checks above. Unblocker (unchanged): licensed Vivado
`report_timing` plus `write_sdf` on probe designs, per
TIMING_XCKU3P.md section 5. License state UNKNOWN; no check run.
Until then STA runs on the preliminary ballpark DB and every report is
labeled estimate. No delay number was invented to get around this.
