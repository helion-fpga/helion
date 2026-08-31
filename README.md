# Helion Design Suite (1.0)

Original FPGA family + CAD. Native `aarch64-apple-darwin`. No vendor bitstream.

**1.0 bar:** `cargo test --workspace` (no board). SystemVerilog subset through synth → pack → PathFinder → bitgen → cycle-accurate fabric sim. 4-bit counter LED = cnt[3].

```
cargo test --workspace
cargo run -p helion-cli -- doctor
cargo run -p helion-cli -- run examples/counter.sv --cycles 16
cargo run -p helion-cli -- report_timing examples/blinky.sv
```

VHDL is 1.1, not a gate. Legal fence: no Project X-Ray, no UNISIM, no AMD/Intel/Lattice backends.

## Flow

| Step | Crate | What it does |
|---|---|---|
| synth | helion-sv | sv-parser + AIG + FlowMap LUT6 |
| pack | helion-pack | LUTFF + IOB + MAC27 + BRAM18 |
| place | helion-place | timing-driven vs wirelength, BLE overflow |
| route | helion-route | PathFinder A* on the tile RR graph |
| sta | helion-sta | create_clock / graph + Manhattan |
| drc | helion-drc | occupancy, unrouted IO, clocks |
| bits | helion-bits | FeatureMap frames + `.hbits` |
| sim | helion-fabric | 6-input IMUX LUT + FF + IOB + STAT |
| hw | helion-hw | IEEE 1149.1 TAP, sim cable |

Tcl client (tree / console / flow rail): `hds::synth`, `read_sv`, `create_clock`.
