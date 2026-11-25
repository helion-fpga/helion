# FFMUX emission scope

Read-only code map for the FFMUX emission lane (UltraScale+ grind).
Base: `fm/ultra-bitgen-ffmux` at `15ec6db2`. Branch:
`fm/ultra-bitgen-ffmux-scope`. No production behavior is changed by
this file. Every anchor below was verified by reading the file on
this branch. Paths are relative to the repo root unless noted.

Status at base: the flow emits no FFMUX line at all. Zero `FFMUX`
hits in `crates/helion-ultrascale/src/fasm_gen.rs` and in
`crates/helion-cli/src/ultra.rs` outside the regression-test doc
comment at `crates/helion-cli/src/ultra.rs:1611-1613`. The six
FFMUXA1 select bits are all clear in the emitted frames, so the FF D
mux is unconfigured. See
`tools/emitter-xczu3eg/EMITTER_XCZU3EG_REPORT.md:138-159` for the
controlled experiments (BYP fixes AQ but route still fails on A_O
and cosim fails at elaboration; D6 gives real values, all 400
inverted).

## a. Flow FASM emission

- `crates/helion-cli/src/ultra.rs:524` `run_impl` - the whole
  `helion impl` flow for an UltraScale+ part; FASM assembly and
  BITGEN live here.
- `crates/helion-cli/src/ultra.rs:602-609` - FF FASM call site:
  `emit_ff_fasm(&packed.slices, &placed, &HashMap::new())`,
  generated before ROUTE, consumed at BITGEN. Comment at 602-607
  records that `helion-ir` `Hff` carries no INIT/SRVAL params, so
  the attribute map is empty and every FF defaults to V0.
- `crates/helion-cli/src/ultra.rs:612-618` - FF control FASM call
  site: `emit_ff_control_fasm(&packed.slices, &placed)`.
- `crates/helion-cli/src/ultra.rs:686-706` - BITGEN assembly: LUT
  FASM at 699-700, then `fasm_lines.extend` for LUT, FF, FF control,
  and PIP lines at 701-705, joined at 706. A new FFMUX line list
  belongs in this extend sequence, next to the FF lines.
- `crates/helion-ultrascale/src/fasm_gen.rs:124-151` `emit_ff_fasm`
  - emits one INIT line plus one SRVAL line per used FF BEL, in
  placed-slice order then BEL index ascending, using the placed site
  name as the FASM tile. The parallel insertion point for a new
  `emit_ffmux_fasm` function.
- `crates/helion-ultrascale/src/fasm_gen.rs:52-57`
  `ff_init_fasm_line` - builds `{site}.{FF}.INIT.V0` or `.V1`;
  `None` defaults to V0 (Xilinx default INIT 0).
- `crates/helion-ultrascale/src/fasm_gen.rs:60-65`
  `ff_srval_fasm_line` - same shape for SRVAL.
- `crates/helion-ultrascale/src/fasm_gen.rs:36-48`
  `ff_fasm_name` - BEL index 0..16 maps to `AFF`..`HFF2`;
  single source of truth is `bitloc::ff_fasm_name`.
- `crates/helion-ultrascale/src/fasm_gen.rs:1-30` - module doc:
  complete-FASM rule, canonical `.V0`/`.V1` spelling, resolved
  polarity `bit = NOT(INIT)`.
- `crates/helion-ultrascale/src/synthmap.rs:671`
  `emit_ff_control_fasm` - per-group CEUSED/SRUSED/CLKINV/SRINV/
  SYNC lines; the existing control-line emitter, same call pattern
  as `emit_ff_fasm`.
- `crates/helion-ultrascale/src/bitgen.rs:1012-1014` - the "V1 is 1"
  comment: enum doc on `LutFfFeature::FfInitV` /
  `FfSrvalV`, "`XFF.INIT.V0` / `.V1`; the bool is the INIT value
  (V1 is 1)".
- `crates/helion-ultrascale/src/bitgen.rs:1023-1090`
  `parse_lut_ff_feature` - parses `.V0`/`.V1` into `value = (v ==
  "V1")`, i.e. V1 means value 1.
- `crates/helion-ultrascale/src/bitgen.rs:87-96` - module doc
  polarity section: INIT 0 selects `.V0`, which SETS the bit;
  INIT 1 selects `.V1`, which CLEARS the bit; `bit = NOT(INIT)`,
  same for SRVAL; a bit reading 1 renders `.V0`.
- `crates/helion-ultrascale/src/bitgen.rs:1308-1440` - parse-back
  render of one SLICE tile's LUT/FF INIT lines; `.V1` render at
  1416 and 1434.
- `crates/helion-ultrascale/src/bitloc.rs:787-801`
  `ff_placed_bit` - placed INIT bit is `value: !init`
  (`bit = NOT(INIT)`); `bitloc.rs:810-824` `ff_init_fasm` -
  V suffix names the INIT value; `bitloc.rs:41-53` module doc
  polarity section.
- How AFF.INIT.V0/V1 lines are produced today: `emit_ff_fasm`
  (`fasm_gen.rs:124-151`) calls `ff_init_fasm_line`
  (`fasm_gen.rs:52-57`) per FF BEL, with `attr.init` from the
  `FfAttr` map (empty in the flow today, so always V0), and
  `bitgen.rs` resolves the line through the segbits DB
  (`bitgen.rs:1795`) or the Y_ofst tables (`bitgen.rs:1111-...`).
- Where an FFMUXA1 line would be emitted: a new emitter parallel
  to `emit_ff_fasm` in `fasm_gen.rs`, called from `run_impl` at
  `ultra.rs:686-706` and extended into `fasm_lines` next to the FF
  lines. Data available at that call site: which FF (packed slice
  `ffs: HashMap<u8, String>`, bel index to cell name, plus placed
  site string), which slice (placed site, e.g. `SLICE_X1Y0` for
  blinky), which letter (`bel / 2`, see `ultra.rs:906-913`).
  NOT available there today: which driving net feeds the FF D.
- Sibling-flow precedent for the decision logic:
  `crates/helion-xilinx7/src/emit.rs:1378-1400` `mux_for_ff` -
  letter-matched mux input (`AX` for AFF, `BX` for BFF, etc.),
  with the carry check first (D from carry selects XOR, never a
  letter input). Note the 7-series spelling differs
  (`AFFMUX.AX`); UltraScale+ uses `FFMUXA1.BYP/D6/...`.

## b. Wire model

- `crates/helion-ultrascale/src/wire_db.rs:33-34` - module doc:
  no public source maps a BEL pin to an INT wire.
- `crates/helion-ultrascale/src/wire_db.rs:410-426`
  `add_pin_wire` - registers one caller-supplied
  (site, bel, pin) to (tile, wire) mapping; fails loud on an
  unknown tile or wire.
- `crates/helion-ultrascale/src/wire_db.rs:518-522`
  `wire_for_bel_pin` - plain lookup in the `pin_wires` map,
  keyed by (site x, site y, bel, pin); returns `None` when
  unregistered.
- `crates/helion-ultrascale/src/fabric.rs:56` - `WireDb` trait
  declaration of `wire_for_bel_pin`.
- `crates/helion-ultrascale/src/tileconn.rs:51-64` - module doc,
  BEL-pin naming: `({L}6LUT, O6)` to `{L}_O`, `({L}Q, Q)` to
  `{L}Q`, and `({L}Q, D)` and `({L}Q2, D)` to `{L}X`: the FF D
  input arrives through the bypass `X` pin (FFMUX BYP), and both
  FFs of a letter share the one `X` pin.
- `crates/helion-ultrascale/src/tileconn.rs:733-800` - the alias
  closure that registers the flow BEL-pin aliases; the FF D
  aliases are at 793-794: `({l}Q, "D")` to `{l}X` and
  `({l}Q2, "D")` to `{l}X`, i.e. for letter A, BEL `AQ` pin `D`
  aliases to site pin `AX`. LUT O6 alias to `{l}_O` is in the
  same loop.
- `crates/helion-ultrascale/src/tileconn.rs:889-890,908-909` -
  test fixture `TINY_TILECONN`: `A_O` stitches to
  `LOGIC_OUTS_E0`, `AQ` to `LOGIC_OUTS_E14`, `AX` to
  `BOUNCE_E_0_FT1`.
- `crates/helion-ultrascale/src/tileconn.rs:998-1027` - test
  `fixture_wire_db_resolves_flow_bel_pins`: `wire_for_bel_pin`
  resolves `A6LUT/O6`, `AQ/D`, `AQ/Q`, and site pin `A1`.
- `crates/helion-ultrascale/src/router.rs:276-293` - the router
  resolves every net's driver pin and every sink pin through
  `wire_for_bel_pin` and fails loud with `NoPath` when any pin
  has no wire. This is where a LUT-O to FF-D net is forced
  through the INT: the driver is an INT wire, the sink is an INT
  wire, and the path between them is INT PIPs.
- `crates/helion-cli/src/ultra.rs:869-1010`
  `build_routable_nets` - builds the router input: a LUT `O`
  endpoint becomes driver pin `O6` (943-945), an FF `Q` endpoint
  becomes driver pin `Q` (947-949), and any other FF endpoint,
  including `D`, becomes a sink (949). Cell-to-BEL letter mapping
  is at 906-913 (LUT letter `b'A' + bel`; FF letter
  `b'A' + bel / 2`, suffix `Q`/`Q2`).
- No intra-slice path is recognized anywhere: a grep for
  intra-slice, site-internal, and same-slice shortcuts across
  `pack.rs`, `place.rs`, `router.rs`, `route_to_pip.rs`, and
  `build_routable_nets` finds none. Pack, place, and route all
  treat a LUT-O to FF-D net as a fabric net. Pack cannot pair
  them either: `PackedUltraSlice` (`pack.rs:72-97`) has no
  `ff_d` field, `pack_ultra` (`pack.rs:203-208`) takes no D-net
  input, and `extract_cells` (`ultra.rs:284-289`) drops the FF
  `D` pin, keeping only CLK/CE/SR.

## c. US-09 checkers under tools/us09v/

Driver: `tools/us09v/us09v/verify.py:74-216` `verify_fasm`
(stages: parse, ports, place, route, extract, cosim; extract
errors abort before cosim at 188-190) and
`tools/us09v/us09v/verify.py:217-267` `verify_bitstream`
(decode, then `verify_fasm`).

### routecheck.py

- `tools/us09v/us09v/routecheck.py:103-258`
  `check_route_fasm` - driver/sink/tree walk over stitched
  fabric nodes.
- `tools/us09v/us09v/routecheck.py:267-289`
  `_slice_output_pins` - output pins of a slice driven by
  configured BELs. A slice Q pin counts as a driver only when
  an FFMUX feature selects it: 274-277 adds `{c}Q`/`{c}Q2`
  only if some `FFMUX{c}{1,2}.` feature is present. The
  `{c}MUX` pin is added only via an OUTMUX feature (280-281).
  The LUT O pin `{c}_O` (`A_O`) is never added: there is no
  branch for it, so a LUT driving the fabric through its `A_O`
  site pin is not counted as a driver, and the net it drives
  fails as undriven / driven-only-by-PIP.
- `tools/us09v/us09v/routecheck.py:291-334`
  `_slice_input_pins` - input pins consumed by configured BELs.
  An FF group counts as used (clock, CE, SR sinks) only if an
  FFMUX feature is present for one of its letters (303-312).

### extract.py

- `tools/us09v/us09v/extract.py:595-660` - FF modeling in
  `_model_slice`. The FFMUX key is built at 604-608
  (`FFMUX{c}{1,2}` per group); the `used` list is built only
  from FFMUX selections. With no FFMUX feature, no FF is
  modeled: no `reg` is emitted, and the Q pins for those
  letters are emitted as `1'bx` at 684-688.
- `tools/us09v/us09v/extract.py:630-631` - `init =
  ff_attr(F, ff, "INIT")`, `srval = ff_attr(F, ff, "SRVAL")`.
- `tools/us09v/us09v/extract.py:646` - `reg {q} = 1'b{init};`
  is the extracted FF's initial value.
- `tools/us09v/us09v/extract.py:686-724` `_mux_sel` - maps a
  mux selection to a Verilog expression: `D6` to the LUT O6
  wire, `D5` to O5, `CY`/`XORIN` to carry wires, `F7F8` to
  the wide mux, `BYP` to `X(c)` at 712-713.
- `tools/us09v/us09v/extract.py:531-536` `X(c)` - the BYP
  input, resolved lazily via `inp(s, f"{c}X", ...)`, i.e. the
  `AX` site pin and whatever the fabric drives into it.
- `tools/us09v/us09v/extract.py:117-119` `sig` - signal name
  is `s_{site}__{pin}`, so the AX driver reference becomes
  `s_SLICE_X1Y0__A_O`.
- `tools/us09v/us09v/extract.py:263-335` `driver` - resolves a
  stitched node to its driver; a site pin with direction OUT
  resolves to `("site", site, pin)`, so the AX node's driver
  is the site pin `A_O`.
- `tools/us09v/us09v/extract.py:358-364` `expr` - renders a
  site driver as `sig(site, pin)`, i.e. `s_SLICE_X1Y0__A_O`.
- Where the undefined wire comes from: the LUT O6 wire is
  declared as `n(c)`, i.e. `s_SLICE_X1Y0__A`, at 523-525
  (`wire {n(c)} = ...`, `wire {n(c + '_O5')} = ...`). No line
  in `extract.py` ever declares a wire named `s_...__A_O`:
  there are zero `A_O` hits in `tools/us09v/us09v/*.py`. So
  when BYP resolves the FF D through the fabric back to the
  LUT O site pin, the emitted Verilog references an undeclared
  wire and cosim fails at elaboration.

### ff_attr INIT polarity mapping

- `tools/us09v/us09v/fasmio.py:104-121` `ff_attr(features, ff,
  attr)` - returns 1 for `.V0`, 0 for `.V1` (doc: "V0 sets the
  bit, V1 clears it, per INTERFACES.md"); the valued spelling
  (`AFF.INIT = 0/1`) returns the value as-is. So under
  `ff_attr`, V0 means INIT 1 and V1 means INIT 0.
- `tools/us09v/us09v/fasmio.py:124-133` `group_attr` - the
  opposite convention for group attributes (CEUSED, SRUSED,
  CLKINV, SRINV): V1 sets, V0 clears.
- This is the opposite of the flow side: `bitgen.rs:1012-1014`
  and `bitloc.rs:41-53` say the V suffix IS the attribute value
  (V1 is INIT 1). The two sides disagree; see section (e).
  The disagreement is observable in cosim: with D6, all 400
  comparisons invert (emitter report lines 153-159).

### cosim.py INIT handling

- `tools/us09v/us09v/cosim.py` has no INIT logic of its own.
  INIT enters only through the extracted netlist: `extract.py:646`
  `reg {q} = 1'b{init};`.
- `tools/us09v/us09v/cosim.py:268-350` `run` - compiles RTL
  and the extracted netlist separately with iverilog, runs both
  with vvp, and compares the output traces cycle by cycle.
- `tools/us09v/us09v/cosim.py:264-266` `CosimResult.ok` -
  no mismatches, no unmapped outputs, `compared > 0`.
- `tools/us09v/tests/test_cosim.py:96-115`
  `test_flipped_ff_init_mismatch` - pins the polarity
  sensitivity: deleting `AFF.INIT.V1` and adding
  `AFF.INIT.V0` on the blinky fixture must produce
  mismatches.

## d. Segbits DB

Files: `crates/helion-ultrascale/data/segbits_clel_l.db`,
`segbits_clel_r.db`, `segbits_clem.db`, `segbits_clem_r.db`
(vendored prjuray-db). Loader: `crates/helion-ultrascale/src/
segbits.rs` (`FileSegbitsDb::lookup` at 378; vendored-load
tests at 628-629, 745-758). FFMUX features resolve through the
generic segbits lookup in the emitter (`bitgen.rs:1795`) and in
parse-back (`bitgen.rs:2519`); there is no FFMUX special case
in `bitgen.rs` or `segbits.rs` (zero `FFMUX` hits in both).

- FFMUXA1 options, `segbits_clel_r.db:669-674` (same bit
  pattern in `segbits_clel_l.db:669-674` and
  `segbits_clem.db:730-735`):
  - `CLEL_R.FFMUXA1.BYP !13_04 !13_05 !14_02 !14_03 !14_05 15_01`
  - `CLEL_R.FFMUXA1.CY !13_04 !13_05 14_02 !14_03 !14_05 !15_01`
  - `CLEL_R.FFMUXA1.XORIN 13_04 !13_05 !14_02 !14_03 !14_05 !15_01`
  - `CLEL_R.FFMUXA1.D5 !13_04 13_05 !14_02 !14_03 !14_05 !15_01`
  - `CLEL_R.FFMUXA1.D6 !13_04 !13_05 !14_02 !14_03 14_05 !15_01`
  - `CLEL_R.FFMUXA1.F7F8 !13_04 !13_05 !14_02 14_03 !14_05 !15_01`
  Each option is 6 bits (frames 13, 14, 15; `!` = must be
  clear, bare = must be set). BYP sets only `15_01`; D6 sets
  only `14_05`. The six options are mutually exclusive
  encodings of the same 6-bit field.
- FFMUXA2 options, `segbits_clel_r.db:675-680` (frames 13, 14,
  15, same shape, different bits).
- All FFMUX lines: `segbits_clel_r.db:669-764`, 96 lines per
  file (16 muxes `FFMUX{A..H}{1,2}` times 6 options), same
  count in `segbits_clel_l.db`, `segbits_clem.db`, and
  `segbits_clem_r.db`.
- AFF.INIT entries, identical in all four CLB DBs
  (`segbits_clel_r.db:17-20`, same lines in the other three):
  - `CLEL_R.AFF.INIT.V0 12_02` (line 17, bit set)
  - `CLEL_R.AFF.INIT.V1 !12_02` (line 18, bit clear)
  - `CLEL_R.AFF.SRVAL.V0 12_01` (line 19, bit set)
  - `CLEL_R.AFF.SRVAL.V1 !12_01` (line 20, bit clear)
  The V0/V1 pair is an exact complement: V0 sets the one bit,
  V1 clears it. The DB itself does not say which V is INIT 1;
  that mapping is the polarity flag in section (e).

## e. INTERFACES.md item 5 (FF INIT polarity flag)

`docs/ultra-bitgen-crew/INTERFACES.md:65-72`, quoted:

> FLAG (interfaces session 2 item 5): the value-to-feature
> mapping (which of V0/V1 is INIT=1) is unverified by any
> on-disk source; FF INIT stays BLOCKED on a polarity
> experiment (a Vivado US+ bitstream with known INIT, or a
> BitFiltrator demo.py run reading the bit value at the
> located FAR and frame_ofst). Until the mapping lands, FF
> INIT bits must not be emitted on the assumption bit=value.
> The V0/V1 suffix does not uniformly encode the attribute
> value: for INIT and SRVAL, V0 sets the bit and V1 clears
> it; for CEUSED, CLKINV, and SRINV, V0 clears it and V1
> sets it.

Current state of the two sides: the flow side
(`bitgen.rs:87-96`, `bitloc.rs:41-53`) treats the V suffix as
the attribute value (V1 is INIT 1, bit = NOT(INIT)), while
the checker side (`fasmio.py:104-121`) treats V0 as INIT 1
and V1 as INIT 0. The flag is unresolved at this base.

## f. Golden fixture

- `tools/us09v/tests/fixtures/blinky/blinky.fasm` (14 lines):
  5 INT PIP lines, `SLICE_X10Y10.A6LUT.INIT[63:0] =
  0x5555555555555555`, `ABCDFF.CEUSED.V0`, `CLKINV.V0`,
  `MODE.FF`, `SRUSED.V0`, `SYNC.ASYNC`,
  `SLICE_X10Y10.AFF.INIT.V1`, `SLICE_X10Y10.AFF.SRVAL.V1`,
  and `SLICE_X10Y10.FFMUXA1.D6`. Note the fixture uses D6
  (intra-slice LUT O to FF D, no fabric route for that net)
  and V1 for INIT/SRVAL.
- `tools/us09v/tests/fixtures/blinky/port_map.json`:
  `clk` to `SLICE_X10Y10.CLK1`, `led` to `SLICE_X10Y10.AQ`.
- `tools/us09v/tests/fixtures/blinky/rtl.v`: `reg q = 1'b0;
  always @(posedge clk) q <= ~q; assign led = q;`
- Generator: `tools/us09v/gen_fixtures.py:204-205`
  `ff_feats` writes `{letter}FF.INIT.V1` and
  `{letter}FF.SRVAL.V1`; the blinky block at 230-258 sets
  `FFMUXA1.D6` and routes `[("AQ", "A1")]` (feedback AQ to
  A1) via `build_fixture` (140-...).
- Expected verify is defined by `tools/us09v/us09v/verify.py:
  74-216` `verify_fasm` (parse, ports, place, route, extract,
  cosim stages) and exercised by
  `tools/us09v/tests/test_verify.py:26-36` (`verify_fixture`
  helper) and `tools/us09v/tests/test_verify.py:42-44`
  (`test_blinky` asserts `rep.ok`).

## g. Regression test blinky_flow_emits_bitstream_on_xczu3eg

`crates/helion-cli/src/ultra.rs:1617`
`blinky_flow_emits_bitstream_on_xczu3eg`. Doc at 1597-1616
states what it pins and deliberately does NOT assert US-09
verify-bit, because the emitted bitstream fails it on route
(no FFMUXA1, no real driver for AQ) and on cosim (extractor
models AQ as X without FFMUX).

What it asserts about the 13-line FASM:

- `ultra.rs:1666-1680` - the exact expected 13 lines:
  `SLICE_X1Y0.A6LUT.INIT[63:0] = 0x5555555555555555`,
  `AFF.INIT.V0`, `AFF.SRVAL.V0`, `ABCDFF.CEUSED.V0`,
  `SRUSED.V0`, `CLKINV.V0`, `SRINV.V0`, and 6 INT_X1Y0 PIP
  lines.
- `ultra.rs:1681` - `assert_eq!(fasm.lines().count(), 13,
  "flow FASM line count")`.
- `ultra.rs:1683-1688` - every expected line must be present
  verbatim.
- `ultra.rs:1690-1698` - every `INT_X` PIP must be on
  `INT_X1Y0` (the provisional INT rule is proven only for
  INT_X1).
- `ultra.rs:1664` - 14,952 FDRI frames; IDCODE `0x04a42093`
  present in the bitstream.
- `ultra.rs:1713` - every stored H-1 band checks Exact over
  all 14,952 frames (no Q residual, no mismatch).
- `ultra.rs:1716-1727` - the standalone decoder round-trips
  all 13 flow lines, and `decoded.unexplained.len()` is
  exactly 38, all inside the H-1 band.
- The test skips unless `HELION_TILECONN_DB_ROOT` and
  `HELION_TILECONN_PART=xczu3eg-sbva484-1-e` are set
  (1617-1635). The sibling test at `ultra.rs:1534`
  (`blinky_fasm_emits_and_parses_back_on_xczu3eg`) pins the
  same 13-line FASM through the CLI bitgen path directly.

Consequence for the implementation worker: adding any FFMUX
line makes the FASM 14 lines, so the count assert at 1681,
the expected array at 1666-1680, and the decoder round-trip
asserts at 1716-1727 must be updated together (new expected
line, count 14, and the unexplained-bit accounting if the
FFMUX bits land outside the H-1 band). The doc comment at
1597-1616 must also be updated: once FFMUX is emitted, the
"deliberately NOT asserted" US-09 note no longer describes
the state, and the worker should say whether verify-bit is
now asserted or still deferred and why.

## Minimal-change list, branch (1): BYP chosen

Ranked smallest first. BYP matches what the router already
does (LUT O to FF D through the INT to the AX site pin), so
the flow side is one new emitter; the checker side carries
the work.

Flow:

1. New `emit_ffmux_fasm` in `fasm_gen.rs` next to
   `emit_ff_fasm` (124-151), emitting
   `{site}.FFMUX{letter}{1,2}.BYP` per used FF BEL, and extend
   it into `fasm_lines` in `run_impl` (`ultra.rs:686-706`).
   The selection needs the FF D driving net: that net is not
   in `PackedUltraSlice` today (section a), so the emitter
   needs either a `ff_d` map threaded from `design.nets`
   (same `pin_to_net` pattern as `sta_on_placed`,
   `ultra.rs:773-799`) or a same-letter rule (FF D comes from
   the same-letter LUT when that LUT drives the D net, the
   7-series `mux_for_ff` pattern at
   `helion-xilinx7/src/emit.rs:1378-1400`).
2. No router change: the route already goes through the INT
   to AX (the BYP shape). No segbits change: `FFMUXA1.BYP`
   already resolves through the generic lookup
   (`bitgen.rs:1795`, DB lines in section d).
3. Update the regression test (`ultra.rs:1617`): expected
   array 1666-1680 gains the FFMUX line, count 13 to 14 at
   1681, decoder asserts at 1716-1727 re-checked.

Checkers:

4. `routecheck.py:267-289` `_slice_output_pins`: count the
   LUT O pin `{c}_O` as a driver when the LUT is live (INIT
   present and the letter's FFMUX or OUTMUX selects it), or
   at minimum when `FFMUX{c}{1,2}.BYP` is present. Without
   this, route still fails on the A_O net (emitter report
   lines 149-151).
5. `extract.py`: declare the `{c}_O` site-pin wire. The LUT O6
   wire is declared as `s_...__{c}` at 523-525 while the BYP
   path references `s_...__{c}_O` via `sig` (117-119) and
   `driver`/`expr` (263-335, 358-364). Minimal fix is one
   alias line per letter mapping the `A_O` signal to the LUT
   O6 wire (or declaring `wire s_...__A_O = <lut o6 wire>`),
   in `_model_slice` next to the LUT declarations. Without
   this, cosim fails at elaboration on the undefined wire.
6. No INIT polarity change is needed for BYP alone: the FF D
   value path is unchanged, only the driver modeling changes.
   But if the BYP experiment is re-run, re-check cosim after
   steps 4-5, because the emitter report's BYP cosim failure
   was at elaboration, before any value comparison ran.

## Minimal-change list, branch (2): D6 chosen

Ranked smallest first. D6 matches the golden fixture
(`blinky.fasm` uses `FFMUXA1.D6`) and needs no checker
modeling change, but it contradicts what the router does
today and it carries the INIT polarity side.

Flow:

1. New `emit_ffmux_fasm` in `fasm_gen.rs` next to
   `emit_ff_fasm` (124-151), emitting
   `{site}.FFMUX{letter}{1,2}.D6` per used FF BEL whose D net
   is driven by the same-letter LUT, extended into
   `fasm_lines` in `run_impl` (`ultra.rs:686-706`). Same
   driving-net data gap as branch (1): the D net is not in
   `PackedUltraSlice`, so thread a `ff_d` map from
   `design.nets` (`ultra.rs:773-799` pattern) or apply the
   same-letter rule from `helion-xilinx7/src/emit.rs:1378-1400`.
2. Router decision: with D6 the LUT-O to FF-D net must NOT be
   routed through the INT. `build_routable_nets`
   (`ultra.rs:869-1010`) must skip (or mark intra-slice) a net
   whose driver is a LUT and whose only sink is the same-slice,
   same-letter FF D, and `router.rs:276-293` must then not
   demand INT wires for it. This is the larger flow change in
   this branch: today no intra-slice path is recognized
   anywhere (section b), so this needs a new concept (a
   same-slice net class) in pack or in `build_routable_nets`,
   plus a legality or STA note that the net is unrouted by
   design. Alternatively the route stays and D6 is emitted
   anyway, but then the emitted FASM claims an intra-slice D
   while the PIPs also drive AX, which is a double-drive of
   the FF D and must be called out, not silently kept.
3. No segbits change: `FFMUXA1.D6` resolves through the
   generic lookup (`bitgen.rs:1795`, DB line
   `segbits_clel_r.db:673`).
4. Update the regression test (`ultra.rs:1617`) as in branch
   (1): expected array, count 13 to 14, decoder asserts.
   Additionally, if the net is no longer routed, the 6 PIP
   lines in the expected array change (the AX leg of the
   route disappears), so the expected PIP list and the
   INT_X1Y0-only check at 1690-1698 must be re-derived from
   the new route, not just appended to.

Checkers:

5. No routecheck change: D6 is already modeled
   (`routecheck.py:274-277` counts Q as a driver when any
   FFMUX feature is present; `extract.py:686-724` maps D6 to
   the LUT O6 wire). The golden fixture passes on D6 today.
6. INIT polarity side (required): with D6, cosim gives real
   values but all 400 inverted (emitter report lines 153-159).
   The root cause is the section (e) disagreement:
   `ff_attr` (`fasmio.py:104-121`) returns 1 for V0 and 0 for
   V1, while the flow (`bitgen.rs:1012-1014`,
   `bitloc.rs:41-53`) treats V1 as INIT 1. One side must
   change after the INTERFACES.md item 5 experiment (a Vivado
   bitstream with known INIT, or a BitFiltrator demo.py run
   reading the bit at the located FAR and frame_ofst). Do not
   flip `ff_attr` or the flow's V mapping on the D6 evidence
   alone: the experiment decides which side is wrong, and the
   fix lands on the wrong side only, with the golden fixture
   (`blinky.fasm` V1 lines, generated by `gen_fixtures.py:
   204-205`) and `test_cosim.py:96-115` updated to match the
   resolved mapping.
7. Until the polarity experiment lands, D6 cannot pass cosim
   end to end even though route and extract pass. That is the
   branch's critical path, and it is external (needs Vivado
   data), unlike branch (1), whose critical path is checker
   code only.
