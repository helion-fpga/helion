# FFMUX Checker-Side Fix Report (W-CHECK)

Branch: `fm/ultra-bitgen-ffmux-check`, base `fm/ultra-bitgen-ffmux` at
`e3b23142`. Worktree: `~/workspace/helion-ultra-bitgen-ffmux-check`.
Date: 2026-11-22. Lane decision implemented: D6 for same-slice
LUT-O to FF-D; the FF INIT polarity fix lands on the CHECKER side,
not the emitter. Only `tools/us09v/` (code, fixtures, generator,
README) and `tools/ffmux-check/` (this report) were touched. No
`crates/` file was touched, no other branch was touched.

No em dashes anywhere.

## 1. Polarity decision: V0 = INIT 0, V1 = INIT 1 (checker side)

`ff_attr` in `tools/us09v/us09v/fasmio.py:104-121` previously
returned 1 for `.V0` and 0 for `.V1`. It now returns 0 for `.V0`
and 1 for `.V1`: the V suffix IS the attribute value, for both
INIT and SRVAL. The valued spelling (`AFF.INIT = 0/1`) is
unchanged. Group attributes (CEUSED, SRUSED, CLKINV, SRINV) keep
the opposite convention (V1 sets, V0 clears) in `group_attr`;
only the FF INIT/SRVAL reading changed.

This decision is made on balance of evidence, NOT settled by a
direct Vivado known-INIT comparison. The evidence, re-examined
before flipping (corpus report Q3, scope sections c and e):

1. FDRE default: the Xilinx FDRE INIT default is 1'b0, and 98.5%
   of used FFs in the 14-pair Vivado corpus read V0 (471,517 of
   478,692). Under the old checker reading, 98.5% of used FFs
   would power up at 1, which contradicts the FDRE default and
   is not plausible for these HLS designs. Under V0 = INIT 0,
   the 1.5% V1 are the registers with an explicit INIT 1, a
   plausible fraction.
2. 7-series ZINI analogy: 7-series prjuray segbits use
   `AFF.ZINI` (bit set) for INIT 0, and the Helion 7-series
   emitter writes ZINI for INIT 0, validated on ZedBoard
   hardware. The UltraScale+ convention (V0 sets the bit, V1
   clears it) matches the same bit-set-means-INIT-0 pattern.
3. Emitter-side consistency: `bitgen.rs` says "V1 is 1",
   `ff_placed_bit` is `!init` (`bitloc.rs:787-801`), and the
   segbits DB has V0 setting the 12_02 bit while V1 clears it.
   The bit-level fact (V0 sets the bit) and the value-level
   reading (V0 is INIT 0) are compatible because the bit is
   NOT the value: bit = NOT(INIT).
4. The old reading was circular, not Vivado data: `ff_attr`
   returned 1 for V0 "pinned by the golden co-sim tests", and
   `gen_fixtures.py` wrote V1 for `reg q = 1'b0`. The fixture
   generator agreeing with `ff_attr` proves nothing about
   Vivado; neither is Vivado data.
5. The D6 cosim inversion (400/400, every comparison inverted)
   is exactly what a checker-side polarity error produces when
   the emitter writes V0 for INIT 0 and the checker reads V0
   as INIT 1. Section 4 confirms it: the same D6 variant now
   gives 400 comparisons, 0 mismatches.

The exact missing datum, still open: a decoded Vivado bitstream
for a design with at least one FF of independently known INIT 1
(and many of known INIT 0) on a decoder-supported part
(xczu3eg), with the FF-to-slice mapping recoverable. Until that
lands, the INTERFACES.md item 5 flag is resolved on balance of
evidence, not by direct proof. That INTERFACES.md text still
states the old mapping and was NOT edited here (outside this
lane's `tools/us09v/` scope); the coordinator should update it
to the resolved mapping and record the missing datum.

## 2. The ff_attr change

`tools/us09v/us09v/fasmio.py`: `ff_attr` flipped as above, with
a code comment stating plainly that the mapping is decided on
balance of evidence (FDRE default 1'b0, 7-series ZINI analogy,
corpus V0 dominance, emitter-side consistency) and NOT settled
by a direct Vivado known-INIT comparison, naming the missing
datum. The comment also records the bit-vs-value distinction
(V0 sets the segbits bit, V1 clears it; bit = NOT(INIT)) so a
later reader does not "fix" the value reading back from the
segbits bit.

Consequential doc updates in the same lane, same reading:
`tools/us09v/us09v/extract.py` module doc (FF polarity line)
and `tools/us09v/README.md` (FF INIT/SRVAL bullet).

## 3. Fixture and test updates

The polarity flip is applied consistently to every fixture
whose registers reset to 0, not only blinky: the old V1
spelling meant INIT 0 under the old `ff_attr`, so leaving any
of them at V1 would make the extractor read INIT 1 against
RTL that starts at 0.

- `tools/us09v/tests/fixtures/blinky/blinky.fasm`:
  `AFF.INIT.V1` becomes `V0`, `AFF.SRVAL.V1` becomes `V0`
  (`reg q = 1'b0` in `rtl.v`). SRVAL flips with INIT because
  the generator writes both from the same `ff_feats` and the
  flow writes both as V0 for INIT 0; SRVAL is unused in this
  fixture (SRUSED.V0) so the SRVAL flip changes no cosim value,
  but leaving it at V1 would assert SRVAL 1 for a register
  whose reset value is 0.
- `tools/us09v/tests/fixtures/counter4/counter4.fasm` and
  `tools/us09v/tests/fixtures/carry8-inc/carry8-inc.fasm`:
  every `*FF.INIT.V1` / `*FF.SRVAL.V1` becomes `V0` (4 FFs in
  counter4, 8 in carry8-inc; both RTLs declare their counters
  `= 0`). No other line in any fixture changed.
- `tools/us09v/gen_fixtures.py`: `ff_feats` now writes
  `{letter}FF.INIT.V0` / `{letter}FF.SRVAL.V0`, so regeneration
  stays consistent with the flipped fixtures. Regeneration was
  run and then reverted: blinky and carry8-inc regenerate
  byte-identical to the edited fixtures, but counter4 does NOT
  (the route-finder BFS picks a different route for one net:
  `BYPASS_E8`/`BOUNCE_E_2_FT1` legs change). The checked-in
  fixtures were therefore restored to the V-flip-only edit;
  the generator nondeterminism is a pre-existing issue, not
  caused by this lane. Log: `~/workspace/ffmux-check-logs/
  regen.log`.
- `tools/us09v/tests/test_cosim.py:96-115`
  (`test_flipped_ff_init_mismatch`): now deletes
  `AFF.INIT.V0` and adds `AFF.INIT.V1` to flip INIT 0 to 1.
- `tools/us09v/tests/test_fasmio.py`: `ff_attr` expectations
  flipped (V0 is 0, V1 is 1, for INIT and SRVAL).
- `tools/us09v/tests/test_extract.py:119`
  (`test_missing_ff_init_errors`): now deletes
  `AFF.INIT.V0`.
- `tools/us09v/tests/test_placecheck.py:72-73` left alone: it
  uses `AFF.INIT.V1` / `AFF.SRVAL.V1` only as filler features
  on a synthetic place tile; placecheck never reads the value.

The fixture still passes `verify_fasm` with 0 mismatches
(see section 5: `test_verify.py::test_blinky` green).

## 4. .FWD/.REV bidirectional BYPASS PIP normalization

Corpus incidental finding, now real in the checker: 18,890 PIP
features across the 14 decoded pairs carry a fourth component
(10,389 `.FWD`, 8,501 `.REV`), all bidirectional BYPASS PIPs.
Recounted from the pairs on disk for this report:
18,890 total, 10,389 FWD, 8,501 REV, and no other 4-component
PIP form. The us09v PIP readers matched `PIP.<dst>.<src>`
exactly, so they missed all of them, likely contributing to
undriven-sink counts.

One shared helper, `parse_pip(feat)` in
`tools/us09v/us09v/fasmio.py`, returns the normalized
(dst, src): plain `PIP.D.S` means S drives D; `PIP.D.S.FWD`
means the same; `PIP.D.S.REV` means D drives S, i.e. plain
`PIP.S.D`. The segbits DB confirms the semantics
(`PIP.BYPASS_E8.INT_NODE_IMUX_5_INT_OUT0.FWD` sets 71_13
74_24, `.REV` sets 71_13 and clears 74_24). Adopted at every
PIP read site in `tools/us09v/us09v/`:

- `routecheck.py`: `PIP_RE` accepts the optional suffix and
  the collection loop normalizes (REV swaps) and dedupes, so a
  plain spelling and its `.FWD` spelling on one tile count as
  one PIP, not a double driver.
- `extract.py` `driver()`: a PIP driving `wire` from `src` is
  recognized in all three spellings (`PIP.wire.src`,
  `PIP.wire.src.FWD`, `PIP.src.wire.REV`).
- `extract.py` `_wire_consumed()`: uses `parse_pip` and compares
  the normalized source.
- `ports.py` `trace_pad_to_slice_pin()`: uses `parse_pip` and
  compares the normalized destination.

Parsed tiles are NOT rewritten: the normalization happens at
each read site, so `parse_fasm` output stays faithful to the
input FASM. `gen_fixtures.py --check` was left alone: it only
verifies generated fixtures, which never carry suffixes.

New tests: `parse_pip` unit tests in `test_fasmio.py`
(plain, FWD keeps direction, REV swaps, non-PIP forms return
None); routecheck tests in `test_routecheck.py` (rewriting
every blinky PIP to `.FWD` leaves the route report identical;
plain / `.FWD` / `.REV` spellings of one bidirectional PIP
give the same PIP count and violations; plain plus `.FWD` on
one tile counts once); an extract test in `test_extract.py`
(rewriting every blinky PIP to `.FWD` still rebuilds with no
errors). The `.REV` extract path is covered by the shared
`parse_pip` logic and the routecheck test, not by a direct
extract test: no fixture PIP is bidirectional, so no fixture
PIP has a valid `.REV` spelling.

## 5. D6 re-run result

Original run (`~/workspace/emitter-xczu3eg-logs/flow2/
us09v-d6.log`): cosim 400 comparisons, 400 mismatches, 0 RTL
unknowns, 0 net unknowns; extracted `reg` was `1'b1`.

Re-run, exact reproduction: the decoded blinky FASM (82 lines,
no FFMUX) plus `SLICE_X1Y0.FFMUXA1.D6`, fixture blinky RTL
(`reg q = 1'b0`), port map `clk` to `SLICE_X1Y0.CLK1`, `led`
to `SLICE_X1Y0.AQ`, 200 cycles, seed 1. Log:
`~/workspace/ffmux-check-logs/us09v-d6-rerun-decoded.log`,
variant: `~/workspace/ffmux-check-logs/d6-variant-decoded.fasm`,
script: `~/workspace/ffmux-check-logs/rerun_d6_decoded.py`.

```
blinky-d6-decoded: FAIL
  [ok] parse: 2 tiles, 83 features
  [ok] ports: 1 inputs, 1 outputs from port map
  [ok] place: 1 slices, 0 carry slices; no violations
  [FAIL] route: 6 PIPs, 3 nets; undriven sink: PIP INODE_W_1_FT1.LOGIC_OUTS_W0 source on INT_X1Y0 on net {INT_X1Y0.LOGIC_OUTS_W0, CLEM_X1Y0.CLE_CLE_M_SITE_0_A_O}; net {INT_X1Y0.BOUNCE_W_0_FT1, CLEM_X1Y0.CLE_CLE_M_SITE_0_AX, INT_TERM_B_X1Y0.SOUTHBUSOUT_FT0_2} is driven only by a PIP, not reachable from any real driver; net {INT_X1Y0.INODE_W_1_FT1, INT_TERM_B_X1Y0.SOUTHBUSOUT_FT0_9, INT_X1Y0.INODE_W_BLN_62_FT1} is driven only by a PIP, not reachable from any real driver
  [ok] extract: 1 sites modelled; no errors; 18 warnings
  [ok] cosim: 400 comparisons, 0 mismatches, 0 RTL unknowns, 0 net unknowns; no mismatches
overall ok: False
```

Cosim is fixed: 400 comparisons, 0 mismatches (was 400
mismatches). The extracted `reg` is now `1'b0` and SRVAL
`1'b0`. The same result holds for the shorter flow-FASM
variant (13 lines plus D6, 14 features, 0 warnings):
`~/workspace/ffmux-check-logs/us09v-d6-rerun.log`, cosim
400 comparisons, 0 mismatches.

Overall `verify_fasm` is still FAIL, on route only, with
violations word-identical to the original D6 log. That route
failure is the pre-existing checker gap from SCOPE section c
(the route checker does not count the LUT O pin `{c}_O` as a
driver, so the A_O net is undriven and two PIP nets are
driverless). It is unchanged by this lane and out of this
lane's scope (polarity plus FWD/REV normalization); it needs
the `_slice_output_pins` fix, which belongs to the BYP/route
work, not to the D6 polarity fix.

## 6. Suite counts

Baseline (before this lane, same worktree at `e3b23142`):
75 tests, OK, 113.871 s. Log:
`~/workspace/ffmux-check-logs/baseline-suite.log`.

After this lane: 83 tests, OK, 154.488 s. Log:
`~/workspace/ffmux-check-logs/suite-after.log`. The 8 new
tests are the 4 `parse_pip` unit tests, the 3 routecheck
FWD/REV normalization tests, and the 1 extract `.FWD` test;
all 8 pass, and all 75 pre-existing tests still pass. The
longer run time is the added cosim/extract work in the new
tests, not a regression (same machine, same suite shape).

Rust suites (helion-ultrascale, helion-cli) were NOT re-run:
this lane touches only Python under `tools/us09v/` plus this
report, which no Rust crate imports, so those suites cannot
be affected by the change. No `crates/` file was modified
(verified by `git status`: only `tools/us09v/` and
`tools/ffmux-check/` paths).

## 7. Remaining items (not this lane)

- Route still fails on the D6 variant (section 5): the route
  checker does not count the LUT O pin `{c}_O` as a driver.
  Needs the `_slice_output_pins` fix (SCOPE section c, item 4
  of the BYP branch); that is route-checker work for the
  BYP/route lane, not the D6 polarity fix.
- INTERFACES.md item 5 text still states the old mapping;
  needs a coordinator update to the resolved mapping plus the
  named missing datum (section 1). Outside this lane's scope.
- The missing datum itself (section 1): a decoded Vivado
  bitstream with known INIT on xczu3eg. The nearest on-disk
  candidate, spiOverJtag P-02 on xcku3p, is still blocked on a
  missing xcku3p tileconn/segbits DB.
- `gen_fixtures.py` route-finder nondeterminism (section 3):
  counter4 regenerates with different PIP legs. Pre-existing,
  flagged here, not fixed here.
