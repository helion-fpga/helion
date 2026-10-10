# Backend trait & frozen NetlistIR (milestone M1) — DONE

The implement flow is driven through the `Backend` trait (`crates/helion-backend`),
not through the `helion-pack` / `helion-place` / `helion-route` / `helion-bits`
free functions directly:

```rust
pub trait Backend {
    fn name(&self) -> &str;
    fn pack(&self, netlist: &NetlistIr) -> Result<Packed, String>;
    fn place(&self, packed: &Packed) -> Result<Placed, String>;
    fn route(&self, placed: &Placed) -> Result<Routed, String>;
    fn bitstream(&self, routed: &Routed) -> Result<Bitstream, String>;
}
```

- **`NetlistIr`** (`pub type NetlistIr = Design` in `helion-ir`) is the frozen
  input contract. The full stability contract — arch-neutral primitive set,
  backend assumptions, known gaps, and the semver change policy — is documented
  on the alias itself: `crates/helion-ir/src/lib.rs`.
- **`Hl10tBackend`** (`helion-backend`) ports the existing HL10T flow to the
  trait with zero QoR movement: same free functions, same placer/route defaults
  the CLI used before (`timing_weight` 0.0/0.75, `max_iters` 8/24 by size, IOB
  trim, empty-design bitgen fallback), same diagnostic lines in the same order.
- The CLI (`helion-cli` `compile_design_xdc`) drives the flow through
  `&dyn Backend`. QoR gate unchanged: `crates/helion-cli/tests/qor.rs`
  (gold: counter `WNS_PS=9640`).

Future backends (roadmap M2+) implement `Backend` against the frozen IR. This
crate stays architecture-neutral: no vendor data, no vendor backends.
