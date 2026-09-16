# CAD stages and SOFT law

Not a tutorial. The Session is a stage machine. The IDE paints it. Closing WNS on
an unmapped cone is a bug.

Canonical HTML: [docs/stages.html](https://helion-fpga.github.io/helion/stages.html).
Roadmap voice: [`ROADMAP.md`](../ROADMAP.md).

**Gold lock:** empty-XDC `examples/counter.sv` stays **WNS_PS=9640** (4 LUTFF, 1 IOB,
LED `0000000111111110`).

## Stages

Illegal order refuses. Prerequisites, in this order:

| Stage | Crate | What |
|---|---|---|
| elaborate/map | `helion-sv` / `helion-vhdl` / `helion-hls` | RTL → HNF (LUT/FF/BRAM18/MAC27) |
| pack | `helion-pack` | LUTFF + IOB + MAC27 + BRAM18 |
| place | `helion-place` | sites from HAD, not a hardcoded die |
| route | `helion-route` | PathFinder A* |
| sta | `helion-sta` | WNS/TNS; clock from user XDC/SDC or a default period |
| bitgen | `helion-bits` | FeatureMap `.hbits` |

Optional, after bitgen (or instead of a cable): `sim` (`helion-sim` / `helion-fabric`,
labeled overlay) and `lab` (`helion-lab`). No DONE without live TDO.

Tcl Session commands (`synth_design`, `place_design`, `route_design`,
`write_bitstream`, …) drive these stages. They are not a second CAD.

## SOFT ≠ PASS

Incomplete mapping is named SOFT (name + span + child softs). Ports-only shells
stay `cells=0` / `no_body`. PASS never includes soft cones. STA must not print a
closed WNS on unmapped logic.

Same law in `helion-sv` / `helion-vhdl` and the STA gate (`crates/helion-cli/tests/gold_lab.rs`).
The IDE must surface it. Do not invent LUTs for `unused_*` / `fcov_`. Do not add a
chat overlay that narrates a closed WNS the engine did not earn.

## Stranger clone

```
git clone https://github.com/helion-fpga/helion.git
cd helion
cargo test --workspace
cargo run -p helion-gui --bin helion-ide -- --headless examples/counter.sv
```

Headless gold must print **WNS_PS=9640**. No board. rustc 1.85.

Legal fence: no Project X-Ray, UNISIM, vendor Tcl as product names, AMD/Intel/Lattice
backends. Helion-MM / Helion-ST only.
