# FFMUX flow report (W-FLOW)

Branch: `fm/ultra-bitgen-ffmux-flow`, off `fm/ultra-bitgen-ffmux` at
`e3b23142`. Worktree: `~/workspace/helion-ultra-bitgen-ffmux-flow`.
Date: 2026-11-22. Logs: `~/workspace/ffmux-flow-logs/`.

Decision (from the coordinator, grounded in
`tools/ffmux-corpus/FFMUX_CORPUS_REPORT.md` Q2): emit D6 when the FF
D is the same-slice LUT O (same letter); emit BYP when the FF D comes
from outside the slice. Corpus: same-slice LUT-O to FF-D is D6 in
119,729 cases and BYP with a fabric route from the same LUT O in
exactly 7 of 456,354 rows; BYP is the shape for D from elsewhere
(244,662 BYP rows fabric-fed). No segbits change was needed: FFMUX
lines resolve through the generic segbits lookup (`bitgen.rs:1795`);
for FFMUXA1, BYP sets only `15_01` and D6 only `14_05`.

## What changed

### 1. FFMUX emitter (`crates/helion-ultrascale/src/fasm_gen.rs`)

New `FfMuxSel` enum (`D6`, `Byp`), `ffmux_fasm_line` (BEL index to
`FFMUX{letter}{1,2}`, same letter/index mapping as `ff_fasm_name`),
and `emit_ffmux_fasm` next to `emit_ff_fasm`: one
`{site}.FFMUX{letter}{1,2}.{D6|BYP}` line per used FF BEL, in
placed-slice order then BEL index ascending. A FF cell missing from
the selection map fails loud; the map is built for every packed FF
by construction. Exported from the crate root (`lib.rs`).

### 2. Selection decision (`crates/helion-cli/src/ultra.rs`)

New `ffmux_selections(design, packed)`: recovers the FF D net from
`design.nets` via the `pin_to_net` pattern (same as `sta_on_placed`),
finds the net's LUT-O driver, and compares packed slice index and
BEL letter (`bel / 2` for FFs, `bel` for LUTs). Same slice and same
letter means D6; anything else (other letter, other slice, FF Q,
carry output, port, or no D net) means BYP. `PackedUltraSlice` was
left without an `ff_d` field and `extract_cells` still drops D; the
decision is computed at the emission site instead, which is the
smaller change. Called from `run_impl` next to the other FF FASM
generation and extended into `fasm_lines` at BITGEN assembly, next
to the FF lines.

### 3. Router skip (`build_routable_nets`, same file)

New concept: intra-slice LUT-O to FF-D edges are not fabric nets.
When a LUT O drives a FF D in the same placed slice with the same
BEL letter, that sink is dropped from the routable net; a net whose
sinks are all intra-slice is dropped entirely, with a loud
`hang_diag` line naming the net and the D6 reason. Other sinks on
the same net still route, so the LUT O still reaches the fabric
where it is consumed elsewhere, and the FF D is never
double-driven (no PIPs drive its AX pin for that net). Only LUT O
drivers are filtered: an FF Q or carry output driving an FF D
still routes to AX (BYP). No change to `router.rs`: the router
never sees the edge.

## New blinky FASM (12 lines, was 13)

```
SLICE_X1Y0.A6LUT.INIT[63:0] = 0x5555555555555555
SLICE_X1Y0.AFF.INIT.V0
SLICE_X1Y0.AFF.SRVAL.V0
SLICE_X1Y0.FFMUXA1.D6
SLICE_X1Y0.ABCDFF.CEUSED.V0
SLICE_X1Y0.ABCDFF.SRUSED.V0
SLICE_X1Y0.ABCDFF.CLKINV.V0
SLICE_X1Y0.ABCDFF.SRINV.V0
INT_X1Y0.PIP.INT_NODE_IMUX_42_INT_OUT1.LOGIC_OUTS_W14
INT_X1Y0.PIP.BYPASS_W5.INT_NODE_IMUX_42_INT_OUT1
INT_X1Y0.PIP.INT_NODE_IMUX_48_INT_OUT0.BYPASS_W5
INT_X1Y0.PIP.IMUX_W10.INT_NODE_IMUX_48_INT_OUT0
```

13 lines became 12, not 14: the added FFMUXA1.D6 line (+1) is
outweighed by the two AX-leg PIPs that disappeared (-2):
`INT_X1Y0.PIP.INODE_W_1_FT1.LOGIC_OUTS_W0` and
`INT_X1Y0.PIP.BOUNCE_W_0_FT1.INODE_W_1_FT1`. The 4 remaining PIPs
are the AQ feedback leg (LUT input A1). The flow log confirms the
skip: `hang_diag ultra route skip net=q_d reason=intra-slice
LUT-O to same-letter FF-D (D6, no fabric route)`, `route nets=1
iters=1 overused=0`, `route-to-pip pips=4`.

## Decoder round-trip: unexplained bits 38 to 41, all in-band

Verified, not assumed: the release flow test failed first on the
old count (left 41, right 38), so the count was re-derived from the
actual decode. `helion-bitgen decode` on the emitted bitstream
lists 41 unexplained bits, every one at absolute frame bit
1440..=1486, i.e. inside the H-1 band (1440..=1487), and the
decoder does round-trip `SLICE_X1Y0.FFMUXA1.D6` (the D6 bit itself
is explained). The count moved because the H-1 band code is
computed from frame contents: dropping the 2 AX-leg PIP bits and
adding the FFMUX bit changed which band bits are set. The
invariant is not the count but that every unexplained bit is a
band bit and the band check is Exact on every frame (14,952/14,952
in the test). The regression test now pins 41 with that
explanation in its doc comment.

## Regression test updates (`crates/helion-cli/src/ultra.rs`)

`blinky_flow_emits_bitstream_on_xczu3eg`: expected array 13 to 12
lines (added FFMUXA1.D6, removed the 2 AX-leg PIPs), count assert
13 to 12, unexplained-bit assert 38 to 41, doc comment updated.
Same pins otherwise: IDCODE `0x04a42093`, 14,952 FDRI frames, band
Exact on all frames, decoder round-trips every flow line.
US-09 verify-bit is still deliberately NOT asserted: the flow now
emits FFMUXA1.D6, so the route leg's missing-driver failure is
gone on the flow side, but cosim still depends on the unresolved
FF INIT polarity flag (INTERFACES.md item 5), whose fix belongs to
the checker lane (`tools/us09v/`), not this flow. That lane was not
touched.

The sibling bitgen-path test
`blinky_fasm_emits_and_parses_back_on_xczu3eg` is unchanged (it
inlines the old 13-line FASM as a bitgen fixture); only its comment
was corrected to say it pins the bitgen path, not the current flow
FASM.

New unit tests (5): `fasm_gen` gains `ffmux_lines_use_letter_and
_index_from_bel`, `ffmux_missing_selection_fails_loud`,
`ffmux_names_cover_all_sixteen`; `ultra.rs` gains
`ffmux_selections_same_letter_lut_is_d6` and
`ffmux_selections_other_sources_are_byp` (one LUT O driving three
FF D pins: same slice same letter is D6, same slice other letter
is BYP, other slice is BYP; the shared net still routes with only
the D6 sink dropped). The existing
`io_and_clock_nets_skip_loudly_not_error` was updated: the fixture
q net is now intra-slice, so zero nets route.

## Suites

Baselines (from the lane brief): ultrascale 423/0/1, CLI 70/0/1.
Counts below are recounted from the suite logs in
`~/workspace/ffmux-flow-logs/` (release, `-j1`, tileconn env set
so the flow test runs instead of skipping).

- helion-ultrascale: 426 passed, 0 failed, 1 ignored
  (baseline 423/0/1 + 3 new `fasm_gen` FFMUX tests).
  Log: `ultrascale-suite.log`, `ULTRASCALE_EXIT=0`.
- helion-cli: 72 passed, 0 failed, 1 ignored (baseline 70/0/1 + 2
  new `ffmux_selections` tests). Log: `cli-suite-noenv2.log`,
  `CLI_EXIT=0`. The full CLI suite runs without the tileconn env
  (the baseline condition); the flow test itself was run separately
  with the env set (`release-flow-test2.log`, `EXIT=0`).

Note on a misleading intermediate run: running the full CLI suite
WITH the tileconn env set fails 2 tests
(`blinky_stops_at_route_without_pin_wire_map`,
`counter_stops_at_route_without_pin_wire_map`), because those two
tests assume no pin-wire map exists. That is a pre-existing
test/env interaction, not a regression: they pass without the env,
as at baseline. The same with-env run also showed the counter
hitting its known pre-existing ROUTE failure (OveruseTimeout,
wire 36, tile (1,0), 3 nets, 50 iters), unchanged by this lane.
A stale `/tmp/helion-us09-flow-blinky/blinky.xcku3eg.bit` left by
that with-env run then failed the noenv rerun once
("bitstream file exists despite the ROUTE failure"); removing the
stale files fixed it, and the clean rerun is the count above.

## Flow run (`helion impl examples/blinky.sv --part xczu3eg`)

Command (release binary, tileconn env
`HELION_TILECONN_DB_ROOT=~/workspace/prjuray-db-ref/zynqusp`,
`HELION_TILECONN_PART=xczu3eg-sbva484-1-e`):

```
./target/release/helion impl examples/blinky.sv --part xczu3eg \
  --out ~/workspace/ffmux-flow-logs/flow-run/blinky.xczu3eg.bit \
  --fasm-out ~/workspace/ffmux-flow-logs/flow-run/blinky.xczu3eg.fasm
```

- Exit code: 0.
- Bitstream: 5,982,532 bytes (same as the emitter-lane blinky).
- FDRI frames: 14,952 (asserted by the regression test).
- IDCODE `0x04a42093` present (asserted by the regression test).
- FASM: the 12 lines above, in
  `~/workspace/ffmux-flow-logs/flow-run/blinky.xczu3eg.fasm`.
- Full log: `~/workspace/ffmux-flow-logs/flow-run/stdout.log`.
- Decoded FASM and unexplained-bit listing:
  `~/workspace/ffmux-flow-logs/flow-run/decoded.fasm`,
  `~/workspace/ffmux-flow-logs/flow-run/decoded-unexplained.log`.

## Notes and open items

- No missing datum: D6 emission needed nothing beyond public
  data (corpus census, segbits DB, tileconn DB).
- FF INIT polarity (INTERFACES.md item 5) is untouched here, by
  design: the corpus report recommends fixing the checker-side
  `ff_attr`, which is W-CHECK's lane.
- The counter design was not run: the lane task asked for blinky
  only, and the counter still has its known pre-existing ROUTE
  failure (OveruseTimeout) on the base branch.
- No em dashes in any changed file or in this report.
