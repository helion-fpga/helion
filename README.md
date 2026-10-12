<p align="center">
  <img src="docs/brand/lockup.png" alt="Helion" width="420">
</p>

<p align="center">
  <strong>The full FPGA flow in one Rust binary: synth, pack, place, route, timing, bitstream.</strong>
</p>

<p align="center">
  <a href="https://github.com/helion-fpga/helion/releases/latest">Download</a> &bull;
  <a href="https://helion-fpga.github.io/helion/">Docs</a> &bull;
  <a href="CHANGELOG.md">Changelog</a> &bull;
  <a href="ROADMAP.md">Roadmap</a> &bull;
  <a href="CONTRIBUTING.md">Contributing</a>
</p>

<p align="center">
  <a href="https://github.com/helion-fpga/helion/actions/workflows/ci.yml"><img src="https://github.com/helion-fpga/helion/actions/workflows/ci.yml/badge.svg" alt="CI"></a>
  <a href="https://github.com/helion-fpga/helion/releases/latest"><img src="https://img.shields.io/github/v/release/helion-fpga/helion" alt="Latest release"></a>
  <a href="https://github.com/helion-fpga/helion/releases"><img src="https://img.shields.io/github/downloads/helion-fpga/helion/total.svg" alt="Downloads"></a>
  <a href="LICENSE-APACHE"><img src="https://img.shields.io/badge/license-Apache--2.0%20OR%20MIT-blue.svg" alt="License"></a>
</p>

Helion is an FPGA CAD suite: write SystemVerilog or VHDL, run one command, and get a placed, routed, timing-closed bitstream. No vendor tools, no black boxes. It targets its own HL10T die and Xilinx 7-series parts (xc7a35t today), and it is architected for more: the compiler emits a vendor-neutral netlist IR, and each device family is a backend.

## Screenshots

<p align="center">
  <img src="docs/screenshots/ide-physview.png" alt="Physical View 2.0: device view with heat maps, inspector, and layer controls" width="800"><br>
  <em>Physical View 2.0: congestion and timing heat maps, inspector, layer controls</em>
</p>

<p align="center">
  <img src="docs/screenshots/ide-schematic.png" alt="Schematic with level-of-detail block view on a 12k-cell design" width="800"><br>
  <em>Schematic with level-of-detail blocks on a 12,174-cell Ibex</em>
</p>

<p align="center">
  <img src="docs/screenshots/ide-waveform.png" alt="Waveform viewer with density toggle" width="800"><br>
  <em>Waveform viewer with Comfortable/Compact density</em>
</p>

<p align="center">
  <img src="docs/screenshots/ide-crossprobe.png" alt="Cross-probing a timing path in the device view" width="800"><br>
  <em>Cross-probing: click a timing path, it lights up in the device view</em>
</p>

<p align="center">
  <img src="docs/screenshots/ide-tcl.png" alt="Tcl console" width="800"><br>
  <em>Real Tcl console: synth_design, place_design, write_bitstream, get_cells</em>
</p>

<p align="center">
  <img src="docs/screenshots/ide-qor.png" alt="QoR suggestions engine" width="800"><br>
  <em>QoR suggestions engine with measured impact ranking</em>
</p>

## Why Helion

- **One binary, whole flow.** `helion implement design.prj` runs synthesis → packing → placement → PathFinder routing → STA → bitstream. No toolchain stitching.
- **Deterministic.** Same RTL in, same bits out. The QoR gate fails the build if LUT counts, WNS, or bitstream bytes move without a reason recorded in the same commit.
- **Fail-loud.** Over-capacity designs error instead of emitting a clipped bitstream; congested routing is refused, never silently clipped.
- **Measured, not claimed.** Frontend allocations fell 24.2M → 409k (down 98%) across the perf campaign; the area campaign cut uart 420 → 244 LUTs (down 42%). Every number traces to the benchmark report or a pushed commit.
- **Multi-vendor by design.** `NetlistIR` plus a `Backend` trait. HL10T and Xilinx 7-series are the first two backends, not the only ones.

## Features

**Compile**
- SystemVerilog / VHDL-2008-subset frontend, AIG-based synthesis with ABC-style rewriting (structural hashing, cut-based resynthesis)
- LUT6/FF/DSP/BRAM packing, simulated-annealing placement, PathFinder routing
- Graph-based STA: WNS/TNS, hold checks, per-path reports
- Bitstream generation with hash-verified round trips

**Xilinx 7-series backend (v3.0)**
- xc7a35t device model from open bitstream databases (Project X-Ray)
- Pack, place, route, FASM emission, frames-to-bitstream, all in Rust, zero vendor tools
- FASM output byte-identical to prjxray's encoder; bitstream writer validated against known-good bitstreams

**Verification (v3.0)**
- SAT-based combinational equivalence checking (miter + Tseitin, FFs cut to pseudo-IOs)
- SystemVerilog assertions: immediate and concurrent assert, implication operators, `$rose`/`$fell`/`$stable`
- Clock-domain-crossing structural analysis (11 violation classes), integrated in the IDE

**IDE**
- Physical View 2.0: FPGA resource layers, solo/isolate, inspector, congestion and timing heat maps, find palette, ruler
- Schematic with level-of-detail for large designs, hierarchy browser, waveform viewer
- Cross-probing, Tcl console, design runs with strategy sweep, checkpoints, testbench runner, constraints wizard, QoR suggestions, VCD-driven power, integrated logic analyzer

## Quickstart

```bash
git clone https://github.com/helion-fpga/helion.git
cd helion
cargo run -p helion-cli -- run examples/counter.sv --cycles 16
# LED[16]=0000000111111110, same bits in the fabric model and the event sim
```

## Installation

Prebuilt binaries are on the [releases page](https://github.com/helion-fpga/helion/releases/latest):

- Linux x86_64 (`.tar.gz`)
- macOS arm64 (`.zip` and `.tar.gz`)
- `SHA256SUMS.txt` for verification

macOS note: the app is ad-hoc signed, not notarized. On first launch, clear the quarantine flag once:

```bash
xattr -cr /Applications/Helion.app
```

## Building from source

Prerequisites: Rust 1.85 or newer (install via [rustup](https://rustup.rs)).

```bash
git clone https://github.com/helion-fpga/helion.git
cd helion
cargo build --release
# CLI:  ./target/release/helion
# IDE:  ./target/release/helion-ide
```

Run the test suite:

```bash
cargo test --workspace
```

## Architecture

```
RTL → helion-sv (synth) → NetlistIR → Backend trait
                                       ├─ HL10T: pack → place → route → STA → bitstream
                                       └─ Xilinx 7-series: pack → place → route → FASM → frames → .bit
```

The compiler never knows what a LUT is *for*. `NetlistIR` is vendor-neutral; each device family implements the `Backend` trait (pack, place, route, bitgen). Adding a device means writing a backend, not forking the compiler. Part facts live in HAD (`devices/helion/`), not hardcoded in the compiler.

**Legal fence:** Helion ships no proprietary vendor IP or databases. Vendor backends are built on open bitstream databases only.

## Contributing

See [CONTRIBUTING.md](CONTRIBUTING.md). Bug reports and pull requests are welcome; the CI gate (build, tests, clippy, gold timing) must stay green.

## License

Licensed under either of [Apache-2.0](LICENSE-APACHE) or [MIT](LICENSE-MIT), at your option.
