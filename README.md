<p align="center">
  <img src="docs/brand/die-flow.gif" width="920" alt="HL10T die: current enters the pads, runs the fabric, and leaves">
</p>

# Helion Design Suite

Original FPGA family + CAD (current release **[2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4)**). Native `aarch64-apple-darwin`. No vendor bitstream. **2.0** is shipped; **2.0.x** is post-bar runtime/QoR grind (STA / bitgen / place-legalize / synth / parse) with gold lock unchanged — see [CHANGELOG](CHANGELOG.md) and [perf-2.0.4](docs/perf-2.0.4.md).

[![Release](https://img.shields.io/github/v/release/helion-fpga/helion)](https://github.com/helion-fpga/helion/releases/latest)

**We need people.** One owner, a real CAD, Apache-2.0 OR MIT. If you write Rust,
SystemVerilog, STA, IDE tests, or docs, start at
[Get involved](https://helion-fpga.github.io/helion/get-involved.html).

[Download](https://github.com/helion-fpga/helion/releases/latest)
· [Changelog](CHANGELOG.md)
· [Roadmap](ROADMAP.md)
· [Milestones](https://github.com/helion-fpga/helion/milestones)
· [Docs](https://helion-fpga.github.io/helion/)
· [Contributing](CONTRIBUTING.md)
· [Discussions](https://github.com/helion-fpga/helion/discussions)
· [Good first issues](https://github.com/helion-fpga/helion/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22)
· [Sponsor](https://github.com/sponsors/saksham-45)

**1.0 bar:** `cargo test --workspace` (no board). SystemVerilog / VHDL / C subset through
synth → pack → PathFinder → bitgen → cycle-accurate fabric sim. 4-bit counter LED = cnt[3].

```
cargo test --workspace
cargo run -p helion-cli -- doctor
cargo run -p helion-cli -- run examples/counter.sv --cycles 16
cargo run -p helion-cli -- report_timing examples/blinky.sv
cargo run -p helion-cli -- reports examples/counter.sv
```

Headless gold (IDE): `helion-ide --headless examples/counter.sv` must print **WNS_PS=9640**.

Legal fence: no Project X-Ray, no UNISIM, no vendor Tcl, no AMD/Intel/Lattice backends.
Device facts come from the Helion Architecture Database (HAD), never hardcoded in the CAD.

## Roadmap (1.4 → 2.0 shipped; 2.0.x grind)

[ROADMAP.md](ROADMAP.md) · Pages: [roadmap](https://helion-fpga.github.io/helion/roadmap.html) · Latest: [v2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4)

| Release | Status | What | Milestone |
|---|---|---|---|
| 1.4 | shipped | SV elaborator (W1); gold **WNS_PS=9640** | [1.4](https://github.com/helion-fpga/helion/milestone/1) |
| 1.5 | shipped | PNR/STA (W2) + proj/cli (W4) | [1.5](https://github.com/helion-fpga/helion/milestone/2) |
| 1.6 | shipped | IDE (W5) + lab (W3) | [1.6](https://github.com/helion-fpga/helion/milestone/3) |
| 1.7 | shipped | VHDL (W6) + HAD (W8) | [1.7](https://github.com/helion-fpga/helion/milestone/4) |
| 2.0 | shipped | Public OSS CAD bar ([v2.0.0](https://github.com/helion-fpga/helion/releases/tag/v2.0.0)) | [2.0](https://github.com/helion-fpga/helion/milestone/5) |
| **2.0.x** | current | Post-bar opt ships (STA HashSet, bitgen frame buffer, place-legalize, synth_rtl, parse cache); gold lock unchanged | [v2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4) |

## Flow

| Step | Crate | What it does |
|---|---|---|
| synth | helion-sv | sv-parser + AIG + FlowMap LUT6 |
| vhdl | helion-vhdl | VHDL-2008 elaborator → same LUT/FF path as SV |
| hls | helion-hls | C subset scheduled/bound onto LUT/FF/DSP |
| pack | helion-pack | LUTFF + IOB + MAC27 + BRAM18 |
| place | helion-place | timing-driven vs wirelength, BLE overflow |
| route | helion-route | PathFinder A* with hop delay in the cost |
| sta | helion-sta | XDC via `load_xdc` (see Architecture), WNS + hold |
| drc | helion-drc | occupancy, unrouted IO, clocks |
| bits | helion-bits | FeatureMap frames + sparse `.hbits` (encode/decode) + partial (DFX) |
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

Measured by `helion qor <src>`, which prints every axis below in one line
(`helion report_utilization` / `helion report_timing` report them separately).
`crates/helion-cli/tests/qor.rs` asserts this table **and** compares each axis
against the previous Helion commit, so a LUT, WNS, bitstream-size or wall-time
regression fails the build until the row is updated **in the same commit with a
reason**.

| Design | Source | LUTFF | IOB | WNS_PS | r2r_ps | iob_ps | .hbits B |
|---|---|---|---|---|---|---|---|
| blinky | examples/blinky.sv | 1 | 1 | 9700 | 300 | 220 | 153 |
| counter | examples/counter.sv | 4 | 1 | 9640 | 360 | 220 | 185 |
| hier | examples/hier.sv | 1 | 1 | 9700 | 300 | 220 | 153 |

Gold waveform: `helion run examples/counter.sv --cycles 16` → `LED[16]=0000000111111110`
(LED = cnt[3]), identical in the fabric model and the event simulator.

### QoR change log

| Commit | Design | Was | Now | Why |
|---|---|---|---|---|
| f7d83ea | counter.sv | 4 LUT / WNS 9640 | 4 LUT / WNS 9640 | Baseline recorded when the table landed. |
| 9b864af | all | 272485 B `.hbits` | 272485 B `.hbits` | Dense stream: every frame on the die was written, zeros included. |
| this | counter.sv | 4 LUT / WNS 9640 / 272485 B | 4 LUT / WNS 9640 / 185 B | `.hbits` writes only configured frames, and FDRI runs are chunked so the 16-bit length no longer truncates. LUT and WNS held; `decode_packets` proves the stream still programs the gold waveform. |

Wall time: `helion qor` reports `ELAPSED_MS` for the whole synth → pack → place →
route → STA → bitgen flow (~30 ms per example here); the gate fails above
2000 ms, so a flow that gets slower by an order of magnitude cannot land quietly.

## Download

[GitHub Releases](https://github.com/helion-fpga/helion/releases/latest) publish
versioned builds for each `vX.Y.Z` tag. Current: **[v2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4)**.

| File | What |
|---|---|
| `Helion-2.0.4-macos-arm64.zip` | `Helion.app` (Apple Silicon). Unsigned — Gatekeeper will warn. |
| `helion-2.0.4-aarch64-apple-darwin.tar.gz` | CLI + IDE + HAD + examples |
| `helion-2.0.4-x86_64-unknown-linux-gnu.tar.gz` | Linux CLI + headless IDE |
| `SHA256SUMS.txt` | hashes |

Wildcard `Helion-*-macos-arm64.zip` / `helion-*-…` names also match on [latest](https://github.com/helion-fpga/helion/releases/latest).

How we cut a tag: [`RELEASING.md`](RELEASING.md). Notes: [`CHANGELOG.md`](CHANGELOG.md).

## Run on macOS (Apple Silicon)

Native desktop target is **`aarch64-apple-darwin`**. This repository is developed
and gated on Linux (`x86_64-unknown-linux-gnu` in CI).

**Verified on Linux**

- `cargo test --workspace --offline`
- the headless IDE model (`helion-gui::IdeModel`): Tcl console, flow rail, netlist tree
- `helion-ide --version` / `--doctor` / `--headless` / `--stdin` (no window)
- `helion doctor` prints the compile-time target triple, rustc, and the runtime HAD path
- `scripts/build-macos-app.sh --layout-only` assembles `Helion.app` (Info.plist, icon, HAD, examples)

**Not verified on this host (Linux VM — no Apple SDK, no Mach-O, no display)**

- `cargo build --release --target aarch64-apple-darwin`
- launching `dist/Helion.app` / the eframe window on a Mac
- codesign / notarization / `.icns` via `iconutil`

On an **Apple Silicon Mac** with Xcode Command Line Tools and rustup 1.85+:

```bash
git clone https://github.com/helion-fpga/helion.git
cd helion
git pull --ff-only

rustup toolchain install 1.85.0
rustup default 1.85.0
rustup target add aarch64-apple-darwin

# same suite that is green on Linux
cargo test --workspace

# native GUI + CLI bundle (runs `cargo build --release --target aarch64-apple-darwin`)
./scripts/build-macos-app.sh
open dist/Helion.app
```

The bundle lands at `dist/Helion.app`:

| Path | What |
|---|---|
| `Contents/MacOS/Helion` | windowed IDE (`helion-ide`) |
| `Contents/MacOS/helion-cli` | CLI (named `helion-cli` so APFS does not clash with `Helion`) |
| `Contents/Resources/devices/helion` | HAD (FeatureMap / parts) |
| `Contents/Resources/examples` | `counter.sv`, `blinky.sv`, … |
| `Contents/Info.plist` | `fpga.helion.ide`, arm64-only |

Launch and sanity-check:

```bash
open dist/Helion.app
dist/Helion.app/Contents/MacOS/Helion --version
dist/Helion.app/Contents/MacOS/Helion --doctor
dist/Helion.app/Contents/MacOS/helion-cli doctor
dist/Helion.app/Contents/MacOS/helion-cli run \
    dist/Helion.app/Contents/Resources/examples/counter.sv --cycles 16
```

`helion gui` execs the sibling `helion-ide` / `Helion` binary (same directory as the CLI).

Without the `.app`, from the repo checkout:

```bash
cargo run -p helion-gui --release --target aarch64-apple-darwin --bin helion-ide
cargo run -p helion-cli --release --target aarch64-apple-darwin -- doctor
cargo run -p helion-cli --release --target aarch64-apple-darwin -- run examples/counter.sv --cycles 16
cargo run -p helion-cli --release --target aarch64-apple-darwin -- project examples/blinky.prj
```

`HELION_HAD` overrides the part database (otherwise the binary searches
`Helion.app/Contents/Resources/devices/helion`, then cwd, then the compile-time tree).
Native arm64 only — no Rosetta, no vendor bitstream, no Docker.

## IP packages (`.helion`)

Format-1 text manifests next to catalog HDL (`ip/h_gpio`, `ip/h_uart`, `ip/h_rv32_hb1`).
Bus is **Helion-MM** / **Helion-ST** only (never AXI as a Helion product). See [`ip/README.md`](ip/README.md).

```bash
helion ip list
helion ip show ip/h_gpio/h_gpio.helion
helion project examples/ip_ingest/counter_ip.prj
# .prj: read_ip ip/h_gpio/h_gpio.helion
```

## Program / mpsse-sim / OFL (honesty)

No live FTDI on the Linux box → detect must stay **0 probes** / refuse DONE.
Sim path for TAP STAT without hardware:

```bash
./scripts/ibex-prog-mpsse-sim-smoke.sh   # pin-wrap / reduced / bare under ≤120s
helion-prog --detect                    # OFL on PATH → physical_had=0 when empty
```

`openFPGALoader` on the box is legal OSS only (Debian apt). On macOS: `brew install openfpgaloader` (Homebrew bottle; still 0 probes without FTDI). Never invent Helion TAP STAT; never claim board DONE without live TDO evidence. Notes: [`docs/FM-HEL-TOP-ofl-box.md`](docs/FM-HEL-TOP-ofl-box.md), [`docs/FM-HEL-TOP-tap-ibex-smoke.md`](docs/FM-HEL-TOP-tap-ibex-smoke.md).

## Cap holds (Ibex / pin-wrap)

Under `IBEX_IMPL_CAP_SEC=120` (never uncapped): **imux_skip=0**, **IOB=1**, counter gold **WNS_PS=9640**. SOFT ≠ PASS.
Pin-wrap wall after keep/md + STA index (`4b14490`): **~1.40s**; residual AIG/flowmap **STOP** (no cheap cut) — [`docs/FM-HEL-TOP-aig-flowmap-residual.md`](docs/FM-HEL-TOP-aig-flowmap-residual.md). Place affinity cut: [`docs/FM-HEL-TOP-place-legalize-speed.md`](docs/FM-HEL-TOP-place-legalize-speed.md).

**2.0.4** release `helion impl examples/ibex_pin_wrap.sv` stage ms (vs prior baselines on the release notes axes): parse **416→353**, synth_rtl **670→524**, place legalize **213→23**, imux_skip **158→123**. Full table: [`docs/perf-2.0.4.md`](docs/perf-2.0.4.md).
