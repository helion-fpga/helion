"""us09v: UltraScale+ plan part 3 checkers.

The oracles that decide whether a US-09 bitstream is trusted. This
package shares no code with Helion's backend: it reads the public
prjuray-db database (CC0 1.0) and decoded FASM directly, so it can
judge Helion's output without agreeing with itself.

Checkers:
- placecheck: placement legality from site_types and the tilegrid.
- routecheck: route legality from tileconn and the tile-type PIP lists,
  plus connectivity traced from the decoded bits.
- extract: rebuild a gate-level netlist from the decoded FASM alone
  (CLB: LUT6_2, FFs, F7/F8/F9 muxes, CARRY8) and emit Verilog.
- cosim: co-simulate the rebuilt netlist against the RTL with iverilog.
- brute: brute-force small designs (1-20 cells) over legal placements.

Run as: python3 -m us09v <command> ...  (from tools/us09v/)
Tests: python3 -m unittest discover -s tests -v

No em dashes anywhere.
"""
