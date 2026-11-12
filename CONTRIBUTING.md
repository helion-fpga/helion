# Contributing to Helion

Thanks for your interest. Helion welcomes contributions in Rust,
SystemVerilog, timing analysis, the IDE, documentation and testing. This
guide covers how to get a build, how to make a change that lands, and the
rules every change follows.

Useful links: [website](https://helion-fpga.github.io/helion/) ·
[get involved](https://helion-fpga.github.io/helion/get-involved.html) ·
[roadmap](ROADMAP.md) ·
[good first issues](https://github.com/helion-fpga/helion/issues?q=is%3Aissue+is%3Aopen+label%3A%22good+first+issue%22) ·
[Discussions](https://github.com/helion-fpga/helion/discussions) ·
[Code of Conduct](CODE_OF_CONDUCT.md)

## Before you start

- For anything larger than a small fix, open an issue or a Discussion
  first, so we can agree on the approach before you spend time on it.
- Public vendor user guides are a **capability checklist**, not source to
  copy. Helion's code, formats and device data are original.

## Build and check

You need [rustup](https://rustup.rs) and Git. The repository pins Rust
1.99.1 in `rust-toolchain.toml`; rustup installs it on the first build. On
Linux, install the IDE's system libraries first:

```bash
sudo apt-get install pkg-config libxkbcommon-dev libwayland-dev libusb-1.0-0-dev   # Debian/Ubuntu
sudo dnf install pkgconf libxkbcommon-devel wayland-devel libusb1-devel            # Fedora
```

Then:

```bash
git clone https://github.com/<you>/helion.git
cd helion
cargo test --locked -p helion-rune -p helion-hw          # IDE and hardware tests (required in CI)
cargo run --locked -p helion-rune --bin helion-ide -- --headless examples/counter.sv
# must print WNS_PS=9640
```

While iterating, run one crate and one test rather than the whole
workspace:

```bash
cargo test -p helion-sv --lib TESTNAME
```

The full `cargo test --workspace` is large; CI runs it on Linux and macOS.

## Making a change

1. Fork, then branch from `master`.
2. Keep the change focused: one concern, and one crate when you can.
3. Add or update a test that fails without your change. IDE changes need
   tests in `crates/helion-rune` (unit tests or scene snapshots in
   `tests/snap.rs`).
4. Keep the gold: `helion-ide --headless examples/counter.sv` prints
   `WNS_PS=9640`, and `helion run examples/counter.sv --cycles 16` prints
   `LED[16]=0000000111111110`. If your change moves a QoR number on purpose,
   update `crates/helion-cli/tests/qor.rs` in the same commit and say why.
5. Run `cargo fmt` and `cargo clippy` on the crates you touched.
6. Add a line under the unreleased section of [`CHANGELOG.md`](CHANGELOG.md)
   for anything a user would notice.
7. Open a pull request and fill in the template. CI must be green.

Commit messages: an imperative subject line of about 70 characters or less,
optionally prefixed with the area (`sta:`, `feat(rune):`), and a body that
explains why when it is not obvious.

## Rules for the IDE

- **No echo-only controls.** A control does real work through the engines,
  or it renders disabled with the reason.
- CAD work runs off the UI thread (background jobs), never inside a frame.
- Tests never write outside the temp directory and never touch a connected
  board (test builds hide real FTDI devices).
- `third_party/rune` is vendored: never edit it in place. Change Rune
  upstream and re-vendor with `scripts/sync-rune.sh`.

## Legal fence

Every change must stay inside it:

- Original Helion architecture and CAD only. No proprietary vendor IP,
  libraries or databases. Vendor-family backends are built only on open
  bitstream documentation (Project X-Ray, IceStorm, Trellis).
- No vendor trademarks as product names. No UNISIM libraries or vendor Tcl
  scripts copied in.
- No AXI interconnect as Helion IP; Helion's buses are Helion-MM and
  Helion-ST.
- `unsafe` only in documented islands.
- Never attach vendor bitstreams or encrypted vendor IP to issues, PRs or
  releases.

## Where things live

| Area | Path |
|---|---|
| SystemVerilog frontend and synthesis | `crates/helion-sv` |
| VHDL frontend | `crates/helion-vhdl` |
| High-level synthesis | `crates/helion-hls` |
| Netlist IR (HNF) | `crates/helion-ir` |
| Pack, place, route | `crates/helion-pack`, `helion-place`, `helion-route` |
| Static timing, XDC, CDC, power | `crates/helion-sta` |
| DRC and methodology | `crates/helion-drc` |
| Bitstreams | `crates/helion-bits` |
| Device families | `crates/helion-backend`, `helion-ice40`, `helion-ecp5`, `helion-xilinx7`, `helion-targets` |
| Simulation and verification | `crates/helion-sim`, `helion-fabric`, `helion-verif`, `helion-lib` |
| Hardware and debug | `crates/helion-hw`, `helion-debug`, `helion-lab` |
| IP and block design | `crates/helion-ipxact`, `helion-bd`, `ip/` |
| Projects, Tcl session, QoR | `crates/helion-proj` |
| CLI (`helion`) | `crates/helion-cli` |
| IDE (`helion-ide`) | `crates/helion-rune`: views in `src/views`, Tcl in `src/tcl.rs`, runs in `src/runs.rs`; status in `SWEEP.md` |
| UI framework (vendored) | `third_party/rune` |
| HL10 device data (HAD) | `devices/helion/` |
| Examples | `examples/` |
| Website | `docs/` (static HTML, published by `.github/workflows/pages.yml`) |
| Engineering notes (not published) | `notes/` |

## Where help is most useful

- SystemVerilog and VHDL coverage: constructs that still do not lower
  ([#14](https://github.com/helion-fpga/helion/issues/14)).
- Hardware: verifying native FTDI programming on a board, and loading iCE40,
  ECP5 and 7-series bitstreams on real hardware
  ([#17](https://github.com/helion-fpga/helion/issues/17)).
- Timing-model calibration against measured data.
- The open IDE gaps in [`crates/helion-rune/SWEEP.md`](crates/helion-rune/SWEEP.md).
- Docs, the website, and testing on macOS and Linux.

## Releases

Releases are cut from `master` by a maintainer, following
[`RELEASING.md`](RELEASING.md).

## License

By contributing, you agree that your contributions are licensed under the
project's license, [AGPL-3.0-or-later](LICENSE).
