# us09v: UltraScale+ part 3 checkers

The oracles that decide whether a US-09 bitstream is trusted. They
share no code with Helion's backend (except the public prjuray-db
database reads), so they can judge Helion's output without agreeing
with itself. Same principle as x7v for 7-series.

## Layout

- `us09v/db.py`: prjuray-db access (tilegrid, tileconn, tile/site
  types, segbits, package pins). License is verified on every load:
  the clone's COPYING must be the CC0 1.0 legal code, else it fails
  loud. Default root: `~/workspace/prjuray-db-ref/zynqusp`
  (override with `US09V_DB_ROOT`).
- `us09v/fasmio.py`: FASM parsing (plain, valued, grouped
  `A6LUT.INIT[63:0]`, per-bit `ALUT.INIT[i]`), FF/group attributes.
- `us09v/fabric.py`: routing fabric: tileconn-stitched nodes plus
  the PIP graph from tile-type JSONs.
- `us09v/extract.py`: rebuild: netlist from decoded FASM alone
  (LUT6_2, FFs, F7/F8/F9, CARRY8), emitted as Verilog. No access to
  the original placed netlist.
- `us09v/placecheck.py`: placement legality from FASM: site type,
  capacity, control sets per half CLB, carry-chain contiguity.
- `us09v/routecheck.py`: route legality from FASM: every PIP
  exists, no wire with two drivers, every sink driven, every net
  one tree from a real driver.
- `us09v/cosim.py`: co-simulation against RTL (iverilog only):
  same stimulus model and two-samples-per-cycle comparison as x7v.
- `us09v/ports.py`: port resolution: XDC package pins to extractor
  ports (inputs from the IOB "I" pin, outputs traced back through
  PIPs to the driving slice pin), and explicit slice-pin port maps.
- `us09v/verify.py`: end-to-end: decode a `.bit` with the decoder
  worker's `helion-bitgen decode` binary, then parse, place, route,
  extract, co-sim. A design passes only when every check passes.
- `us09v/brute.py`: enumerate every legal placement of 1-20 cells
  on a small slice grid, with exact counts.
- `gen_fixtures.py`: generate the five hand-built fixtures in
  `tests/fixtures/` (RTL + complete FASM + BFS-found INT PIP routes
  + port map). Re-run after any DB change.
- `tests/`: unittest suites, one per checker, including negative
  tests. Run: `python3 -m unittest discover -s tests -v`

## Usage

```sh
# Verify a decoded FASM file against RTL, explicit port map:
python3 -m us09v verify-fasm --fasm tests/fixtures/blinky/blinky.fasm \
  --design blinky --rtl tests/fixtures/blinky/rtl.v --top blinky \
  --port-map tests/fixtures/blinky/port_map.json \
  --workdir ~/workspace/us09-checkers-logs/blinky --cycles 200

# Decode a .bit file and verify it (XDC mode):
python3 -m us09v verify-bit --bit design.bit --design blinky \
  --rtl blinky.sv --top blinky --xdc blinky.xdc \
  --workdir ~/workspace/us09-checkers-logs/blinky --cycles 200

# Count legal placements:
python3 -m us09v brute --cells 2 --kind LUT --grid 2x2
```

## Checker inventory

| checker | verifies |
| --- | --- |
| parse | FASM parses; feature/value spellings understood |
| provisional | flags golden xcku3p designs (no tilegrid entry) |
| ports | every RTL port maps to a real pad or slice pin; outputs trace back to a slice pin |
| place | site type (CLE vs CLEM), capacity (8 LUT / 16 FF / 1 CARRY8 per slice), control sets per half (1 CLK, 1 SR, 2 CE nets), carry chains contiguous and vertical (CIN from the slice directly below) |
| route | every PIP exists in its tile type; no net with two drivers; every active PIP source and every used site input driven; every PIP-driven net reachable from a real driver (no driverless loops). A PIP-existence violation on one tile does not abort the net checks on the verified PIPs. Tiles with config features only (e.g. a BRAM site tile) are not route violations. |
| extract | rebuild from FASM alone: LUT6_2 (O6/O5), FFs (FFMUX/OUTMUX, CE/SR/CLKINV/SYNC), F7/F8/F9, CARRY8 (SINGLE_CY8/DUAL_CY4); LUTRAM/SRL, DSP/BRAM/URAM/clock sites fail loud with the block named. Site identity uses absolute tilegrid names (SLICE_XxYy), never the tile-type relative names (X0Y0). |
| cosim | rebuilt netlist matches RTL over random stimulus, two samples per cycle; mismatches name cycle, bit, RTL value, net value |

## Assumptions (pinned, not validated)

- A-F7F8: wide-mux mapping F7AMUX = AX?B:A, F7BMUX = CX?D:C,
  F7CMUX = EX?F:E, F7DMUX = GX?H:G, F8AMUX = BX?F7B:F7A,
  F8BMUX = FX?F7D:F7C, F9MUX = HX?F8B:F8A; per-letter mux input
  A->F7A, B->F8A, C->F7B, D->F8A, E->F7C, F->F8B, G->F7D, H->F9.
  Every use warns. Needs a Vivado F7/F8/F9 build or UG574 figure
  confirmation.
- A-DI0: an unrouted CARRY8 DI bypass pin reads 0. Pinned by the
  carry8 golden's stated intent (8-bit incrementer). A routed
  bypass pin takes the routed net: the extractor resolves the DI
  bypass through the fabric, so a route that drives the bypass pin
  changes DI. Fixture routes never step on a non-endpoint site
  pin's node for exactly this reason.
- A-CINPIP: the site pin CIN reaches the CIN wire only through the
  CLE PIP `PIP.CLE_CLE_L_SITE_0_CIN_PIN.CLE_CLE_L_SITE_0_CIN`.
  A carry chain's upper slice needs that PIP in its FASM for the
  checker to resolve CIN to the slice below's COUT; without it the
  checker reports CIN undriven.
- A-CTRL: control pins per half: ABCD half CLK1/SRST1, EFGH half
  CLK2/SRST2; CE pins CKEN1-4 map to ABCD-Q, ABCD-Q2, EFGH-Q,
  EFGH-Q2. DIVERGENCE: the crate's legality.rs applies stricter
  per-slice limits (1 CLK, 1 SR, 2 CE per slice); this checker
  applies the per-half rule from the brief. Flagged to the
  coordinator, not resolved here.
- FF INIT/SRVAL: the V suffix IS the value (V0 is 0, V1 is 1);
  the segbits bit is NOT the value (V0 sets the bit, V1 clears
  it, bit = NOT(INIT)). Group attributes CEUSED/SRUSED/CLKINV/
  SRINV: V1 sets, V0 clears. Decided on balance of evidence
  (FDRE default 1'b0, corpus V0 dominance, emitter consistency);
  NOT settled by a direct Vivado known-INIT comparison, whose
  missing datum is a decoded Vivado bitstream with known INIT
  on a decoder-supported part.
- LUT O6/O5 indexing: A1 is the LSB.
- INT PIP FASM order is DST.SRC: `PIP.BOUNCE_E_0_FT1.INODE_E_1_FT1`
  means physical direction INODE_E_1_FT1 -> BOUNCE_E_0_FT1.

## Known gaps (blocked, labeled)

- CLK config, DSP48E2, URAM288: no public encoding; configured
  sites fail loud with the block named. A clock routed directly
  from an IOB pad (no BUFG) works. A BUFG/BUFGCE/MMCM clock is
  NOT modelled (decision, Nov 21: the brief stops spending on
  CLK; a minimal BUFGCE model would need the clock network, which
  is the blocked scope). Clock pins whose driver resolves to an
  unmodelled clock site count as driven for place; the extractor
  emits clock = X for an unmodelled clock driver, so co-sim
  reports net unknowns instead of silently passing.
- Timing is preliminary: no delay model, no timing check.
- BRAM content: place/route checks run, rebuild fails loud
  (no BRAM model). BRAM site tiles (RAMB18_XxYy) carry config
  features only in the decoded FASM; their PIPs are not verified
  (the route checker skips site-named tiles, like SLICE tiles).
- IOB: not modelled; pads are primary ports (inputs) or traced
  slice pins (outputs) only.
- xcku3p goldens are provisional: no xcku3p tilegrid in any public
  DB, so site-type checks are notes and tile types are inferred
  from tile name prefixes. Goldens are bit-diff fixtures, not
  complete routed designs: they fail route (undriven sinks) and
  extract (undriven inputs) with the reasons named.
- F7/F8/F9 mapping (A-F7F8) is assumed, not validated.
- The per-half control-set rule diverges from legality.rs (see
  A-CTRL above).
- Incomplete decodes fail loud at every stage: on P-11 (decode
  exit 1, 70801 unexplained bits) the place checker reports 157
  CIN-undriven and 48 CLK2-undriven slices, because the decoded
  FASM is missing the CIN PIPs and the CLK2 (CTRL_W5) PIPs for
  those slices. That is the oracle working, not a checker bug:
  the named gaps are decoder gaps (unblocker: the decoder worker
  must explain those bits). The route checker on the same decode
  reports the net incompleteness (hundreds of thousands of
  undriven sinks and PIP-only nets) instead of passing.
- The counter4 fixture's LUT input assignment is pinned by the
  golden INITs: C 0x7878 computes C3^(C1&C2), D 0x7f80... computes
  D4^(D1&D2&D3), so the sum bit goes on the highest input of each
  LUT. The original assignment (sum bit on the lowest input) was
  wrong and co-sim caught it.

## Verification (Nov 21, 2026)

- Suite: 75 tests, OK (`python3 -m unittest discover -s tests`,
  log `~/workspace/us09-checkers-logs/unittest-06.log`, 328.5s).
  Includes negative tests per checker: broken placement (overlap,
  capacity, control set, carry-chain gap), broken route (bogus
  PIP, double driver, undriven sink, driverless PIP loop),
  wrong netlist (flipped LUT INIT, flipped FF INIT, missing
  OUTMUX, missing FF INIT, LUTRAM blocked).
- Fixtures: all five hand-built fixtures (and2, blinky, counter4,
  carry8-inc, wide-mux-f7) pass place, route, extract, and co-sim
  (200 cycles, seed 1) end to end.
- Harness goldens (8 files, blinky/counter/carry8/bram on xcku3p
  and xcvu3p): all FAIL with named reasons, as expected for
  bit-diff fixtures. Place passes (1 slice, provisional noted);
  route fails on undriven sinks; extract fails on undriven LUT
  inputs. Log `~/workspace/us09-checkers-logs/golden/golden-verify.log`.
- Public xczu3eg pairs (14 .bit files): decode + check each with
  `python3 batch_pairs.py`, summary
  `~/workspace/us09-checkers-logs/pairs/pairs-summary.json`.
  Every pair's decode exits 1 (unexplained bits), so no pair
  passes end to end; the checkers report the incompleteness per
  design (place/route/extract violation counts by kind in the
  summary). P-11 detail: decode 70801 unexplained bits, 10256
  tiles, 851448 features, 8820 slices, 185 carry slices.
- Flow worker bitstreams (blinky/counter): not available at the
  time of this report; the coordinator hands them over for a
  follow-up verify-bit run.

## Environment notes

- iverilog 12.0 is required for co-sim. It was installed from a
  cached .deb on Nov 21, 2026 and does NOT persist across VM
  replacements; reinstall with `apt-get install -y iverilog`.
- Logs go under `~/workspace/us09-checkers-logs/`, never /tmp.

No em dashes anywhere in this tree.
