<p align="center">
  <img src="docs/brand/die-flow.gif" width="920" alt="HL10T die: current enters the pads, runs the fabric, and leaves">
</p>

# Helion

An original FPGA family and a CAD that owns the whole path: **RTL → pack → place → route → timing → bitstream → sim**. Written in Rust. Native on Apple Silicon. Apache-2.0 OR MIT.

Current release: **[v2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4)**.

[Download](https://github.com/helion-fpga/helion/releases/latest)
· [Docs](https://helion-fpga.github.io/helion/)
· [Changelog](CHANGELOG.md)
· [Roadmap](ROADMAP.md)
· [Contribute](CONTRIBUTING.md)
· [Good first issues](https://github.com/helion-fpga/helion/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22)

If you write Rust, SystemVerilog, STA, IDE tests, or docs: [get involved](https://helion-fpga.github.io/helion/get-involved.html).

## Try it

```bash
git clone https://github.com/helion-fpga/helion.git
cd helion
cargo test --workspace
cargo run -p helion-cli -- doctor
cargo run -p helion-cli -- run examples/counter.sv --cycles 16
```

You should see `LED[16]=0000000111111110` (LED is `cnt[3]`). Same bits in the fabric model and the event sim.

Headless gold (the number that must not silently move):

```bash
cargo run -p helion-gui --bin helion-ide -- --headless examples/counter.sv
# must print WNS_PS=9640
```

Need rustc **1.85**. CI is Linux; the desktop app is **Apple Silicon only** (`aarch64-apple-darwin`).

## What is in the box

| You write | Helion does |
|---|---|
| SystemVerilog, VHDL-2008 subset, or a C subset | Synth onto LUT6 / FF / DSP / BRAM |
| SDC / XDC (`create_clock`, delays, false path, …) | Graph STA: WNS, hold, reports |
| `helion` CLI or the Mac app | Pack, PathFinder route, `.hbits`, fabric sim |
| Tcl (`synth_design`, `place_design`, `write_bitstream`, …) | The same engines, not a vendor tool |

Tcl Session also does `get_cells` / `get_nets` / `set_property` / `mark_debug` / `eco` / `write_checkpoint`. A `.hckp` on disk restores the same bitstream hash.

**Legal fence:** original Helion ISA and CAD only. No Project X-Ray, no UNISIM, no vendor Tcl, no AMD / Intel / Lattice backends. Part facts live in HAD (`devices/helion/`), not hardcoded in the compiler.

## Download

From [Releases](https://github.com/helion-fpga/helion/releases/latest):

| File | What |
|---|---|
| `Helion-2.0.4-macos-arm64.zip` | `Helion.app` (unsigned — Gatekeeper will warn) |
| `helion-2.0.4-aarch64-apple-darwin.tar.gz` | CLI + IDE + HAD + examples |
| `helion-2.0.4-x86_64-unknown-linux-gnu.tar.gz` | Linux CLI + headless IDE |
| `SHA256SUMS.txt` | hashes |

How we cut a tag: [`RELEASING.md`](RELEASING.md).

### Mac app from source

```bash
rustup toolchain install 1.85.0
rustup default 1.85.0
rustup target add aarch64-apple-darwin
./scripts/build-macos-app.sh
open dist/Helion.app
```

| Path | What |
|---|---|
| `Contents/MacOS/Helion` | windowed IDE |
| `Contents/MacOS/helion-cli` | CLI (named so APFS does not clash with `Helion`) |
| `Contents/Resources/devices/helion` | HAD |
| `Contents/Resources/examples` | `counter.sv`, `blinky.sv`, … |

`HELION_HAD` overrides the part database. No Rosetta, no Docker, no vendor bitstream.

## QoR (HL10T-C32-1, 10 ns clock)

`helion qor <src>` prints every column. `crates/helion-cli/tests/qor.rs` fails the build if a LUT, WNS, bitstream size, or wall-time regresses without updating this table **in the same commit, with a reason**.

| Design | Source | LUTFF | IOB | WNS_PS | r2r_ps | iob_ps | .hbits B |
|---|---|---|---|---|---|---|---|
| blinky | `examples/blinky.sv` | 1 | 1 | 9700 | 300 | 220 | 153 |
| counter | `examples/counter.sv` | 4 | 1 | **9640** | 360 | 220 | 185 |
| hier | `examples/hier.sv` | 1 | 1 | 9700 | 300 | 220 | 153 |

Wall time for that flow is ~30 ms here; the gate fails above 2000 ms.

## Flow (crates)

synth (`helion-sv`) → VHDL (`helion-vhdl`) / HLS (`helion-hls`) → pack (`helion-pack`: LUTFF, IOB, MAC27, BRAM18) → place → PathFinder route → STA (`load_xdc`) → DRC → FeatureMap `.hbits` → fabric sim / event sim → IEEE 1149.1 TAP (`helion-hw`).

More: [Architecture](https://helion-fpga.github.io/helion/architecture.html) · [User guide](https://helion-fpga.github.io/helion/use.html).

## IP

Format-1 `.helion` next to catalog HDL (`ip/h_gpio`, `ip/h_uart`, …). Bus is **Helion-MM / Helion-ST** only — never AXI as a Helion product. See [`ip/README.md`](ip/README.md).

```bash
helion ip list
helion project examples/ip_ingest/counter_ip.prj
```

## Program a board

No cable → **refuse DONE**. Never invent TAP STAT.

```bash
helion-prog --detect     # 0 probes if nothing is plugged in
```

`openFPGALoader` is optional (`brew install openfpgaloader`). Notes: [`docs/FM-HEL-TOP-ofl-box.md`](docs/FM-HEL-TOP-ofl-box.md).

## Where we are

**2.0** is the public bar (clone, test, gold 9640, legal fence). **2.0.x** is measured runtime work on the same gold — latest numbers in [`docs/perf-2.0.4.md`](docs/perf-2.0.4.md). Still open: public-widen SOFT (not renamed PASS), and a real probe that blinks an LED. See [ROADMAP](ROADMAP.md) and [issues](https://github.com/helion-fpga/helion/issues).
