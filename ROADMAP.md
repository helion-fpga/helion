# Helion roadmap

Helion is an original FPGA architecture (HL10) and an in-repo CAD suite that
targets it and other open-documented FPGA families. This file says what the
next release delivers, what comes after it, and what has shipped. The
website mirrors it at
[helion-fpga.github.io/helion/roadmap.html](https://helion-fpga.github.io/helion/roadmap.html);
GitHub [milestones](https://github.com/helion-fpga/helion/milestones) track
the issues.

**Gold lock.** The empty-XDC `examples/counter.sv` must report
`WNS_PS=9640` (4 LUTFF, 1 IOB) and simulate to `LED[16]=0000000111111110`
at every step. A change to these numbers lands only in a commit that
updates the QoR tests and says why.

| Release | Status | Theme |
|---|---|---|
| 1.4 to 1.7 | shipped | SystemVerilog elaborator, PnR/STA, project/CLI, first IDE, lab, VHDL, HAD |
| 2.0 | shipped | Public open-source CAD bar |
| 2.0.x | shipped | Runtime and QoR grind without moving gold |
| 2.1 | shipped | Performance and area campaigns, IDE rebuild |
| 2.5 | shipped | IDE features: cross-probing, Tcl console, runs, checkpoints, ILA |
| 3.0 | shipped | Xilinx 7-series backend; SAT equivalence, SVA, CDC |
| **4.0** | **in preparation** | **Rune IDE, iCE40 and ECP5 backends, self-contained build, native JTAG on real hardware** |

## 4.0: what it delivers

The details are in the [changelog](CHANGELOG.md#400--unreleased).

- **A new IDE.** `helion-ide` is rebuilt on the Rune UI framework: a
  Vivado-style shell with Flow Navigator, Sources, Summary, device view with
  I/O planning and pblocks, schematic, timing, waveform, simulation and
  testbenches, coverage, Hardware Manager with ILA/VIO/JTAG-to-AXI and
  configuration memory, IP catalog and block design, HLS, ECO, DFX and
  out-of-context runs, reports, Tcl console, command palette and language
  templates. Every port from the old IDE is tracked in
  [`crates/helion-rune/SWEEP.md`](crates/helion-rune/SWEEP.md).
- **Two Lattice backends.** iCE40-HX8K on Project IceStorm data (`.asc`
  output) and ECP5 LFE5U-85F on Project Trellis data (text config for
  `ecppack`), selectable from the New Project part picker.
- **Verification.** Class-based SystemVerilog testbenches with a UVM
  subset, behavioral VHDL testbenches, and DUT line and branch coverage.
- **Windowless modes** for scripts and CI: `--headless`, `--source`,
  `--stdin`, `--version`, `--doctor`, on the IDE's own Tcl interpreter.
- **A self-contained build.** Rune is vendored in `third_party/rune`, Rust
  1.99.1 is pinned in `rust-toolchain.toml`, and `Cargo.lock` is enforced
  with `--locked` in CI and releases.
- **Native JTAG on real hardware.** The FTDI MPSSE engine scans a real
  ZedBoard chain at 1 to 30 MHz, with a Digilent pin profile and
  falling-edge TDO sampling above 15 MHz.
- **The egui IDE is retired.** It stays buildable as `helion-ide-egui`
  until 4.0 merges to `master`, then `crates/helion-gui` is removed.

Release checklist for 4.0: merge `wip/ide-integration` to `master`, remove
the egui IDE, refresh screenshots, run the full workspace tests, tag
`v4.0.0` per [RELEASING.md](RELEASING.md).

## After 4.0

Ordered roughly by priority. None of these are promised for a date.

**Hardware**
- Verify programming over the native FTDI cable on a board (scan is
  verified; programming is not).
- STAT readback when programming through openFPGALoader.
- Load iCE40, ECP5 and 7-series bitstreams on real boards; today they are
  checked against the open toolchains (icepack, ecppack, prjxray), not on
  hardware.

**Quality of results**
- Calibrate the timing models against measured data, then make pack, place
  and route fully timing-driven. The 7-series model is preliminary and not
  correlated.
- Wire the October campaign's library passes (priority-cut mapping, SAT
  sweeping, carry-chain extraction, timing-driven routing, detailed
  placement, path-based analysis) into the default flow, each behind the
  QoR gate.
- Close the residual buckets from the 1,000-design corpus: latch cells,
  OpenTitan primitive wrappers, enable folding.

**IDE**
- The open items in [`SWEEP.md`](crates/helion-rune/SWEEP.md): a clock-region
  layer (needs a region model in HAD), pblock deletion in `.prj`, DFX
  partitions persisted in the project and locked static routes, horizontal
  scrolling in large-file mode, richer VHDL diagnostics.
- Testbench engine limits: 4-state and wider-than-64-bit values, structs,
  covergroups and DPI in class-based testbenches.

**Project**
- Make the full workspace test run a required CI check (it is
  informational today; the IDE, hardware and gold checks are required).
- Notarized macOS builds, once an Apple Developer ID is available.
- More maintainers (see [GOVERNANCE.md](GOVERNANCE.md)).

## Legal fence (every release)

- Original Helion architecture and CAD. Public vendor user guides are a
  capability checklist, never source to copy.
- No proprietary vendor IP, libraries or databases. Vendor-family backends
  are built only on open bitstream documentation (Project X-Ray, IceStorm,
  Trellis).
- No vendor trademarks as product names.
- No AXI interconnect as Helion IP; Helion's buses are Helion-MM and
  Helion-ST.
- Gold `WNS_PS=9640` unless the QoR tests move in the same commit with a
  reason.

## Shipped releases

Full notes for every release are in the [changelog](CHANGELOG.md).

### 3.0: Xilinx 7-series backend, verification

Xilinx 7-series backend (xc7a35t: pack, place, route, FASM, frames, `.bit`,
on Project X-Ray data) behind a new architecture-neutral `Backend` trait.
Verification track: SAT-based equivalence checking,
SystemVerilog assertions, CDC structural analysis. Physical View 2.0. Twelve
review criticals and a deep-recursion stack overflow fixed.

### 2.5: IDE features

Cross-probing, Tcl console, design runs with strategy sweeps, checkpoints,
testbench runner, constraints wizard, QoR suggestions, VCD-driven power and
an integrated logic analyzer.

### 2.1: performance and area

Criterion benchmarks for every engine, ABC-style AIG rewriting, frontend
allocations down 97%, uart 420 to 244 LUTs, IDE rebuild.

### 2.0.x: runtime and QoR grind

Measured STA, bitgen, place-legalize, synthesis and parse cuts without
moving gold. On release `helion impl examples/ibex_pin_wrap.sv`: parse 416
to 353 ms, synth_rtl 670 to 524 ms, place legalize 213 to 23 ms
([table](notes/reports/perf-2.0.4.md)).

### 2.0: public open-source CAD bar

A stranger can clone, run the tests and the headless gold, and trust the
legal fence. Docs, CI (Linux and macOS gold) and the desktop IDE describe
the same session. [Milestone 2.0](https://github.com/helion-fpga/helion/milestone/5).

### 1.4 to 1.7: foundations

- **1.4**, SystemVerilog elaborator: `always`, `case`, functions, memories
  and generates onto Helion LUT/FF; ports-only shells stay `cells=0`, never
  invented gates. [Milestone 1.4](https://github.com/helion-fpga/helion/milestone/1).
- **1.5**, place, route and STA plus project and CLI: timing-driven
  placement and routing, XDC constraints, `.prj` and `.hckp` that survive a
  restart, ECO that still produces a bitstream.
  [Milestone 1.5](https://github.com/helion-fpga/helion/milestone/2).
- **1.6**, IDE and lab: the first desktop `helion-ide`, CAD jobs off the UI
  thread; lab work on FTDI and openFPGALoader with no invented TAP status.
  [Milestone 1.6](https://github.com/helion-fpga/helion/milestone/3).
- **1.7**, VHDL and HAD: a VHDL-2008 elaborator on the same LUT/FF path as
  SystemVerilog; HAD parts beyond `HL10T-C32-1`, with device facts in TOML.
  [Milestone 1.7](https://github.com/helion-fpga/helion/milestone/4).
