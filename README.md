# Helion Design Suite (1.0)

Original FPGA family + CAD. Native `aarch64-apple-darwin`. No vendor bitstream.

**1.0 bar:** `cargo test --workspace` (no board). SystemVerilog / VHDL / C subset through
synth → pack → PathFinder → bitgen → cycle-accurate fabric sim. 4-bit counter LED = cnt[3].

```
cargo test --workspace
cargo run -p helion-cli -- doctor
cargo run -p helion-cli -- run examples/counter.sv --cycles 16
cargo run -p helion-cli -- report_timing examples/blinky.sv
```

Legal fence: no Project X-Ray, no UNISIM, no vendor Tcl, no AMD/Intel/Lattice backends.
Device facts come from the Helion Architecture Database (HAD), never hardcoded in the CAD.

## Flow

| Step | Crate | What it does |
|---|---|---|
| synth | helion-sv | sv-parser + AIG + FlowMap LUT6 |
| vhdl | helion-vhdl | VHDL-2008 subset → same elaborator as SV |
| hls | helion-hls | C subset scheduled/bound onto LUT/FF/DSP |
| pack | helion-pack | LUTFF + IOB + MAC27 + BRAM18 |
| place | helion-place | timing-driven vs wirelength, BLE overflow |
| route | helion-route | PathFinder A* with hop delay in the cost |
| sta | helion-sta | XDC clocks / I-O delay / false path, WNS + hold |
| drc | helion-drc | occupancy, unrouted IO, clocks |
| bits | helion-bits | FeatureMap frames + `.hbits` + partial (DFX) |
| sim | helion-fabric | 6-input IMUX LUT + FF + IOB + STAT |
| sim | helion-sim | event kernel, agrees with the fabric |
| hw | helion-hw | IEEE 1149.1 TAP, sim cable, partial program |

Tcl Session (`helion-gui` / `helion-proj`): `synth_design`, `opt_design`, `place_design`,
`route_design`, `write_bitstream`, `write_hnf`, `get_cells/get_nets/get_pins`, `set_property`,
`report_timing`, `report_utilization`, `open_hw_manager`, `program_hw`, `mark_debug`, `eco`,
`write_checkpoint`, `report_die`. Each command drives the real engine; `.hckp` restore
reproduces the same bitstream hash.

**IR (HNF):** cells/nets/ports carry `DONT_TOUCH`, `mark_debug`, `LOC`. `helion hnf file.sv`
writes a round-trippable netlist. Checkpoints embed HNF. `(* keep *)` / `(* mark_debug *)`
and `for` generate unroll into that IR.

## QoR (HL10T-C32-1, 10.000 ns clock)

Measured by `helion report_utilization` / `helion report_timing`.
`crates/helion-cli/tests/qor.rs` asserts this table, so a LUT or WNS move fails
the build until the row is updated **in the same commit with a reason**.

| Design | Source | LUTFF | IOB | WNS_PS | r2r_ps | iob_ps |
|---|---|---|---|---|---|---|
| blinky | examples/blinky.sv | 1 | 1 | 9700 | 300 | 220 |
| counter | examples/counter.sv | 4 | 1 | 9640 | 360 | 220 |
| hier | examples/hier.sv | 1 | 1 | 9700 | 300 | 220 |

Gold waveform: `helion run examples/counter.sv --cycles 16` → `LED[16]=0000000111111110`
(LED = cnt[3]), identical in the fabric model and the event simulator.

### QoR change log

| Commit | Design | Was | Now | Why |
|---|---|---|---|---|
| f7d83ea | counter.sv | 4 LUT / WNS 9640 | 4 LUT / WNS 9640 | Baseline recorded when the table landed. |
