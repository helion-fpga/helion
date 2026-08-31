# Helion Design Suite (0.1)

Original FPGA family + CAD. Native `aarch64-apple-darwin`. No vendor bitstream.

**0.1 bar:** `cargo test --workspace` (no board). Structural LUT blinky in cycle-accurate fabric sim.

```
cargo test --workspace
cargo run -p helion-cli -- doctor
```

VHDL is 1.1, not a gate. Legal fence: no Project X-Ray, no UNISIM, no AMD/Intel/Lattice backends.
