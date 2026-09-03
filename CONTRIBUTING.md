# Contributing

Helion is original FPGA CAD. Public vendor user guides are a **capability checklist**, not source to copy.

Site (LMMS-style on-ramp): [docs](https://helion-fpga.github.io/helion/) · [get involved](https://helion-fpga.github.io/helion/get-involved.html).

## Legal fence

- Original Helion ISA and CAD only.
- Do **not** add Project X-Ray, vendor Tcl, UNISIM names, or 7-series/UltraScale/Lattice/Intel bitstream backends.
- No AXI interconnect as Helion IP (Helion-MM/ST).
- `unsafe` is forbidden except documented islands (none in 0.1).

## First contribution (LMMS-style path)

1. Talk in [Discussions](https://github.com/helion-fpga/helion/discussions) or pick a **good first issue**.
2. Fork, clone, branch from `master`.
3. Build:

   ```bash
   cargo test --workspace
   cargo run -p helion-gui --bin helion-ide -- --headless examples/counter.sv
   ```

   Empty-XDC counter must print **`WNS_PS=9640`**.
4. One change, one crate when you can. While iterating: **`cargo test -p CRATE --lib TESTNAME`** (one test name).
5. Open a PR. Fill the template. Fix CI.

## Where to look

| Want | Path |
|---|---|
| SV ingest | `crates/helion-sv` |
| STA / XDC | `crates/helion-sta` |
| Place / route / bits | `crates/helion-place`, `helion-route`, `helion-bits` |
| IDE model + tests | `crates/helion-gui/src/ide.rs` |
| IDE paint | `crates/helion-gui/src/bin/helion-ide.rs` |
| Parts | `devices/helion/` |
| Examples | `examples/` |

## What we need help with

- SV ingest of large files (`examples/ysyx_ibex.sv`) — preprocess, skip packages, **no hang**, synth without abort
- Tests for engine-backed IDE panes (not dumps)
- Docs and the `docs/` site
- macOS / Linux CI

Do **not** start a greenfield visual identity or a vendor-Tcl catalog.
