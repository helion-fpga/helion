# US-09: End-to-end flow (Verilog in, bitstream out)

- Track: UltraScale+, flow. The pipeline currently takes hand-written
  FASM in; no real design can flow through it.
- Branch: fm/ultra-bitstream plus the day-1 staging branches
  fm/ultra-packplace, fm/ultra-router, fm/ultra-timing. Worktree:
  ~/workspace/helion-ultra-bitstream.
- Status: not started beyond staging. ROUTE fails loud, WireDb empty,
  translator not written.

## Goal

A Verilog design flows through Helion to a bitstream on xcku3p: synth,
pack, place, route, bitgen, with no hand-written FASM in the middle.

## Done when

A Verilog design (start with the blinky) flows to a bitstream through
Helion on this part, and that bitstream diffs to zero against the
hand-written-FASM bitstream for the same design.

## What is already done

- Staging code exists from the Oct 22, 2026 day-1 crew: pack/place
  staging on fm/ultra-packplace (CARRY8 included), router scaffold on
  fm/ultra-router, timing staging on fm/ultra-timing with a fail-loud
  UNKNOWN delay DB, multiseed proof (10-seed, WNS 8460 to 8820) on
  fm/ultra-multiseed, and a QoR harness extension (100/100 on xcvu3p).
- The bitgen side is proven: emit_bitstream / parse_bitstream with exact
  round trip parse(emit(f)) == f, taking &dyn BitgenDevice.
- The gaps are named precisely:
  - ROUTE in the Helion impl fails loud: WireDb is empty.
  - The router-to-PIP translator is not written; it is gated on the
    WireDb seam.
  - Pack/place are staging code, not production.
  - Timing is preliminary with no real xcku3p delay data. Critic session
    10 flag F3: sta_on_placed always uses preliminary_xcvu3p delays; fix
    when an xcku3p timing DB exists.
  - `helion run` panics on counter.sv; do not use it for verification.
- The QoR campaign's mapper work (US-10) is the synth side of this flow;
  the two meet at the netlist.

## What was tried and what failed

- Day-1 staging only. No end-to-end run has ever happened on this part.
  The bitgen crew deliberately scoped to FASM-in first, and the driver
  works the bitgen pipeline before the flow.

## What to do

1. Build the WireDb: the routing graph for xcku3p, from the public
   sources (BitFiltrator arch JSON has the INT PIP data: 3,778 PIP
   encodings, 3,774 of 3,774 tile PIPs mapped, 0 missing; audit part 2,
   the Vivado get_pips probe, is still pending on the license).
   Reject the RapidWright.dat path: it is under the Xilinx EULA.
2. Write the router-to-PIP translator on the WireDb seam: routed nets to
   INT PIP FASM lines.
3. Promote pack/place from staging to production on the real device file,
   with CARRY8 carried through.
4. Build the xcku3p timing DB (real delays, not preliminary_xcvu3p) and
   fix the F3 flag. Until then, timing numbers are estimates, and every
   report that shows them must say so.
5. Run the blinky Verilog end to end, then the counter, then the US-07
   random designs through the flow (not just through FASM-in).

## Files and commands

- Staging branches: fm/ultra-packplace, fm/ultra-router, fm/ultra-timing,
  fm/ultra-multiseed, fm/ultra-device (7a4146f: bel.rs, device.rs,
  fabric.rs, lib.rs, tests/smoke_place.rs).
- Contracts: docs/ultra-bitgen-crew/INTERFACES.md (BitgenDevice trait:
  part, idcode, tile_type, tile_base_far, tile_names,
  config_order_fars; emit/parse round trip).
- Data: crates/helion-ultrascale/data/ (vendored); BitFiltrator
  ULTRASCALE_PLUS.json (SHA-256 c6320fe7322d918fc3995f9122f62b0f4ab7b
  282b517bf8231d0c63ac70b436c).

## Dependencies

Depends on US-01 (tilegrid) for real placement and on the INT PIP audit
part 2 (Vivado get_pips probe, license). Related to US-10 (synth QoR is
the front of this flow) and ZB-04/ZB-08 (the 7-series flow has the same
shape of problem).

## Rules

- No em dashes anywhere.
- Fail loud. A stage that cannot do its job must error, never emit a
  bitstream that looks right and is wrong.
- Do not use RapidWright.dat. It is under the Xilinx EULA.
- Timing numbers are estimates until the xcku3p DB exists; say so wherever
  they are shown.
