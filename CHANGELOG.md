# Changelog

All notable Helion releases are listed here. Version numbers match
`workspace.package.version` in `Cargo.toml` and Git tags `vMAJOR.MINOR.PATCH`.
Downloadable builds are on [GitHub Releases](https://github.com/helion-fpga/helion/releases).
The format follows [Keep a Changelog](https://keepachangelog.com/en/1.1.0/).

## [4.0.0] — Unreleased

Helion 4.0 replaces the desktop IDE. The new `helion-ide` is built from
scratch on the [Rune](https://github.com/anomalyco/rune) UI framework and
covers the whole design flow, from sources to a programmed board, in one
Vivado-style window. Rune is vendored, so a plain clone builds. The native
JTAG engine now talks to real hardware, and two Lattice backends (iCE40 and
ECP5) join the HL10 and Xilinx 7-series flows. The empty-XDC
`examples/counter.sv` gold is unchanged: `WNS_PS=9640`,
`LED[16]=0000000111111110`.

### Added

- **New IDE (`crates/helion-rune`, binary `helion-ide`).** Shipped in
  `Helion.app`, the release tarballs and `helion gui`.
  - Shell: Flow Navigator, menus, command palette, resizable and hideable
    docks with layout presets, Start page with recent projects and bundled
    examples, background jobs with status-bar progress, light and dark
    themes.
  - Project: New Project wizard with a board picker and a family-grouped part
    picker (every part a backend can implement), `.prj` sources and
    constraint files, Design Runs table with strategies, seed sweeps, a run
    queue, checkpoints and incremental runs, out-of-context module runs,
    project archive and hardware-platform hand-off.
  - Editor: SystemVerilog, VHDL and Tcl highlighting, find and replace,
    outline, parse diagnostics, bookmarks, go to definition, large-file mode,
    and a Language Templates library that inserts at the caret.
  - Device: die view with site-kind layers, congestion and timing-criticality
    heat maps, rulers and zoom to selection; I/O planning with an I/O ports
    table and a package-pin grid; pblocks that steer placement.
  - Schematic: fan-in and fan-out cones, expand and collapse, selection
    history, sheet find, Fit, Mark Debug.
  - Timing: summary, path table and path detail, clocks, clock interaction
    matrix, CDC, endpoint-slack histogram, Timing Constraints pane and a
    five-step Constraints Wizard.
  - Reports: utilization, power (vectorless and VCD-driven), DRC,
    methodology, clock utilization, control sets, SSN, IBIS, QoR
    suggestions, bitstream frames.
  - Simulation and verification: testbench runner, simulation settings,
    waveform viewer with cursors, radix, fixed-point and analog display,
    virtual buses and SPI/UART/AXI-Lite/FSM decoder lanes, saved wave
    layouts (`.wcfg`), simulation debugger (breakpoints, memory, locals,
    forces), code coverage with an editor gutter overlay.
  - Hardware: Hardware Manager tree, Set Up Debug, hardware ILA, VIO
    dashboard, JTAG-to-AXI console, configuration memory (erase, blank
    check, program, verify, read back, boot), JTAG chain card with TCK
    picker and raw IR/DR scans.
  - Design: IP Catalog and IP Integrator block design with connection
    automation, customize and package IP; High-Level Synthesis pane with
    kernel editor, directives, C synthesis and C/RTL co-simulation; ECO
    view; partial reconfiguration (DFX) view.
  - Tcl console on the same engines as the GUI: the full verb set of the
    previous IDE plus run, report, hardware, DFX and ECO verbs.
- **Lattice iCE40 backend** (`helion-ice40`): iCE40-HX8K from the Project
  IceStorm database; pack (LUT6 to LUT4 decomposition), place, A* route,
  global clocks, `SB_CARRY`, `SB_RAM40_4K` with INIT, `SB_PLL40`, and IceStorm
  `.asc` output that round-trips through icepack/iceunpack.
- **Lattice ECP5 backend** (`helion-ecp5`): LFE5U-85F from the Project
  Trellis database; pack, place, A* route over the Trellis mux graph, CCU2
  carry chains, DP16KD block RAM, MULT18X18D, PLL and DCC clocks, and Trellis
  text-config output accepted by `ecppack`.
- **Windowless IDE modes**, all on the IDE's own Tcl interpreter:
  `helion-ide --headless <file>` (full flow plus `report_timing`; prints
  `WNS_PS`), `--source script.tcl` (batch, exit 1 on the first error),
  `--stdin` (line-by-line console), `--version`, `--doctor`, `--help`.
- **Native JTAG engine** (`helion-hw`): IEEE 1149.1 over FTDI MPSSE through
  libusb, with a mock FTDI device for tests. Verified read-only against a
  real ZedBoard (on-board Digilent FT232H): the chain `xc7z020` plus ARM DAP
  enumerates and matches openFPGALoader, stable at 1, 6, 15 and 30 MHz.
  - Digilent cables get their own pin profile, picked from the USB
    descriptor (the generic AN_129 pins left TDO floating).
  - Above 15 MHz TDO is sampled on the falling edge;
    `HELION_JTAG_READ_EDGE=rising|falling` overrides.
  - `open_hw_target -cable auto|sim|usb|native` reaches the native chain
    from Tcl; `examples/native_scan.rs` is a read-only engine check.
  - Programming over the native cable is implemented but not yet verified
    on a board; the IDE says so wherever it offers it.
- ILA, VIO and JTAG-to-AXI run over any JTAG cable (simulated TAP or the
  native FTDI engine) through `helion-debug`.
- Engines added for the IDE: implementable-part registry and family
  dispatch (`helion-targets`: HL10, iCE40, ECP5, and 7-series when the
  Project X-Ray database is present), out-of-context synthesis with a
  checkpoint cache, testbench code coverage, ECO operations with
  incremental implement and ECO bitstream diff, DFX partitions with
  per-module partial bitstreams, pblock placement regions with a `PBLK-1`
  DRC, configuration-memory parts and `write_cfgmem`, `report_control_sets`,
  an IBIS writer, SSN over placed ports, VCD-driven power in `helion-sta`,
  `$urandom`, `$urandom_range` and `randcase` in testbenches, SystemVerilog
  testbenches driving VHDL DUTs, and C/RTL co-simulation in the gate
  simulator, with an FSMD RTL emitter so co-simulation exercises the C
  function. iCE40 and ECP5 runs report family utilization, timing, DRC and
  die geometry.
- **Class-based SystemVerilog testbenches and a UVM subset**
  (`helion-svtb`): classes with inheritance, virtual methods and parameters,
  dynamic arrays, queues, associative arrays, `fork`/`join`, events,
  mailboxes, semaphores, virtual interfaces with clocking blocks, constrained
  randomization, macros and `` `include ``, and a documented `uvm_pkg`
  subset (factory, phases and objections, `config_db`, TLM, sequences,
  reporting). The Simulation pane picks a UVM test (`+UVM_TESTNAME`) and
  shows the UVM report summary. `examples/uvm_counter` verifies
  `counter.sv`. Limits are reported as errors: 2-state values up to 64 bits,
  no structs, covergroups or DPI.
- **VHDL testbenches** (`*_tb.vhd`): a behavioral VHDL simulator with delta
  cycles, `wait on/until/for`, `after` delays, resolved `std_logic`,
  `report`/`assert`, `numeric_std` and `textio` output; SV or VHDL DUTs.
  `examples/counter_tb.vhd` is a self-checking example.
- **DUT RTL line and branch coverage** in the Coverage view and the editor
  gutter, from source-location probes inserted at synthesis.
- `helion-verif`: the ILA and testbench engines, extracted so the IDE and
  CLI share them. `examples/counter_tb.sv` is a self-checking testbench.
- `helion equiv` and `helion synth --verify`: SAT-based equivalence between
  pre- and post-optimization netlists.
- Synthesis and implementation: `Dsp48e1` and `Bram18e1` IR cells with
  pins, DSP48E1 FASM emission for 7-series, NPN-based AIG cut rewriting,
  AIG balancing, timing-driven placement ordered by net criticality, CDC
  2-FF synchronizer and handshake detection, parallel module elaboration.
- A 77-PR engineering campaign (October 2026) added library-level engines
  across synthesis, packing, placement, routing, STA, DRC, simulation,
  formal verification, HLS, hardware debug and the IP catalog. Many of these
  passes are not yet wired into the default `synth → place → route` flow;
  [`notes/reports/CAMPAIGN_REPORT_AND_ROADMAP.md`](notes/reports/CAMPAIGN_REPORT_AND_ROADMAP.md)
  lists them and what remains.

### Changed

- **Rune is vendored** in `third_party/rune` (MIT, upstream commit
  `a7f276c`), with its license. A fresh clone, a CI runner or a source
  tarball builds without a sibling checkout. `scripts/sync-rune.sh`
  re-vendors any upstream commit and records it in
  `third_party/rune/VENDORED.md`.
- **Rust 1.99.1.** `rust-toolchain.toml` pins it and rustup installs it on
  first build. Rust 1.85 can no longer build the workspace.
- Building on Linux needs `pkg-config`, `libxkbcommon-dev`,
  `libwayland-dev` and `libusb-1.0-0-dev`. The Linux `helion-ide` needs
  `libxkbcommon0`, `libwayland-client0` and `libusb-1.0-0` at run time.
- CI installs those packages, builds with `--locked` everywhere, requires
  the `helion-rune` and `helion-hw` tests, and checks the counter gold with
  `helion-ide --headless`. The full workspace test run is informational.
- Release builds use Rust 1.99.1; the Linux tarball now includes
  `helion-ide`, and the release notes list its runtime libraries. The macOS
  `.app` bundle test moved to `helion-rune`.
- **License: AGPL-3.0-or-later** (previously MIT OR Apache-2.0).
  [`TRADEMARKS.md`](TRADEMARKS.md) covers the Helion name and logo.
- Interned net and cell names (`Spur`) across the IR and backends.
- Test builds hide real FTDI devices; `HELION_TEST_REAL_USB=1` opts in.

- An implementation run with unrouted nets or overused routing now logs a
  critical warning and skips the bitstream instead of reporting a clean run;
  Tcl `write_bitstream` applies the same gate (`-force` to overwrite an
  existing file).

### Deprecated

- The egui IDE (`crates/helion-gui`) is now `helion-ide-egui`. It still
  builds on this branch and is removed when 4.0 lands on `master`.

### Fixed

- Hardware ILA lists the debug core's probes instead of the simulation
  probe list.
- JTAG-to-AXI reads that return DECERR no longer report data.
- BSCAN USER2 and USER4 no longer collide on the TAP.
- One test shelled a real openFPGALoader and another sent configuration
  opcodes to a connected cable; both are now impossible in test builds, and
  a poisoned openFPGALoader environment lock no longer fails unrelated tests.

[4.0.0]: https://github.com/helion-fpga/helion/compare/v3.0.0...HEAD

## [3.0.0] — 2026-10-12

Two new product tracks plus a reliability sweep and a major IDE upgrade.

### Added

- Xilinx 7-series backend (zero vendor tools, open bitstream databases only
  via Project X-Ray / prjxray):
  - New `helion-backend` crate: architecture-neutral `Backend` trait with
    associated stage types (`Packed`/`Placed`/`Routed`/`Bitstream`) and the
    pack to place to route to bitstream stage contract.
  - New `helion-xilinx7` crate: xc7a35t device model (18,055 tiles), 7-series
    pack/place/route, human-readable FASM emission, frame resolution, and a
    `.bit` writer. Blinky runs end to end from SystemVerilog to a valid
    bitstream: FASM to frames is byte-identical against prjxray's
    `fasm2frames.py`, and the writer reproduces openXC7's `blinky.bit`
    byte-for-byte on identical frames.
  - `helion-ir`: new `CellKind::IobIn` (input IOB) and `CellKind::ClockBuf`
    (global clock buffer) with HNF round-trip support, plus a `NetlistIr`
    alias as the backend-neutral design input. HL10T pack now rejects these
    cells loudly instead of silently dropping them.
- Verification track:
  - SAT-based combinational equivalence checking (`SolverBackend` trait,
    varisat 0.2.1 backend, Tseitin encoding, LUT6 to AIG bridge, miter
    construction with counterexample reporting).
  - SVA subset: immediate `assert`, `assert final`, concurrent
    `assert property`, `|->`, `|=>`, fixed `##N`, `$rose`/`$fell`/`$stable`,
    with parser, sim engine, and IDE integration (wave markers, Tcl
    `assert_report`).
  - CDC structural analysis in `helion-sta`: clock-domain extraction,
    crossing detection, synchronizer recognition, and all eleven violation
    classes with IDE integration (`report_cdc_crossings`, cross-probing).
    Handshakes and async FIFOs are flagged for review, never declared safe.
- Physical View 2.0: FPGA resource layers with solo/isolate controls,
  congestion and timing-criticality heat maps, inspector panel, find palette
  (Ctrl+K), ruler with tile and delay measurement, and upgraded die
  interaction (route hit-testing, multi-select, hover tooltips).
- UX pass 2: schematic level-of-detail for large designs (block rectangles
  with cell/port counts at fit zoom), hierarchy breadcrumb for large designs,
  waveform density toggle (Comfortable/Compact), scrolling tab bar and
  toolbar overflow menus, right-click context menus (timing rows, device
  canvas, waveforms), syntax highlighting in the sim source view, proper
  empty states, and a real clock-interaction matrix.

### Fixed

- Stack overflow on deep priority chains: all recursive RExpr/Expr walkers on
  the synthesis path are now iterative with heap work stacks; the 50,000-deep
  regression test passes and debug Ibex synthesis no longer crashes.
- All 12 code-review criticals, each with a regression test: binary literal
  sizing, `index_part` small-value decoding, hierarchy cycle infinite
  recursion, mixed-width word-add carry drop, `RExpr::Range` lo/hi order,
  priority-grant heuristic scope, non-constant shift lowering (new
  `RExpr::Shl`), placer seed/site-index invariant, VCD truncation flag,
  generated-clock constraint target port, Tcl `set_property KEEP`, and Tcl
  `string range` out-of-range handling.

[3.0.0]: https://github.com/helion-fpga/helion/compare/v2.5.0...v3.0.0

## [2.5.0] — 2026-10-12

Nine "best in the market" IDE features, toast/macOS hotfixes, and a UX fix batch.

### Added

- Cross-probing: clicking a timing path lights it up in the device view and schematic (probe selection synced across views).
- Real Tcl console: inline console (bottom panel) plus a floating window on Cmd+Backtick; drives the same `Session` engines as the CLI, with expression syntax errors that report line numbers.
- Design runs + strategy sweep: named runs with strategy presets (Default, TimingExplore, RuntimeOpt, PhysOpt, Custom), per-run placer seed / router iters / AIG rewrite / timing-driven / phys-opt toggles, queue with streaming results, seed sweeps, and best-run activation.
- Checkpoints: save/restore named implementation checkpoints; checkpoint manager pane.
- Integrated testbench runner: discover `*_tb.sv` testbenches, run them against the sim kernel (full net resolution for LUT pins, driven top-level inputs, multi-level LUT chains), stream logs with severity coloring.
- Constraints wizard: visual SDC editor — clocks, I/O delays, false paths, and multicycle exceptions with a draft/apply flow.
- QoR suggestions engine: analyzes the implemented design and surfaces prioritized, actionable QoR suggestions with severity levels.
- VCD-driven power estimation: VCD dumper with exact multi-level LUT-chain net resolution, switching-activity power report with severity-graded nets.
- ILA / logic analyzer: sim-side ILA with probe selection, edge/value triggers, waveform viewer (GTKWave-style with cursor/A-B markers), and VCD export marking invalid samples as X.
- Editable source editor (UX batch): click-and-type editing with syntax highlighting, VS Code-style minimap, line numbers, dirty indicator, Cmd+S save, Alt+Z soft-wrap.

### Fixed

- Toast notifications no longer pile up as giant permanent cards: they auto-dismiss after 5s on a wall-clock timer (the old frame-count timer never expired while the UI was idle), are capped at 360px with wrapping, max 4 stacked, deduped, and the "Started: …" spam is gone — job progress already lives in the status bar; toasts are completions/errors only.
- macOS: ad-hoc codesign the `Helion.app` bundle during the release build (and verify it in CI). Unsigned downloads were reported as "damaged" by Gatekeeper on recent macOS; the bundle is now well-formed. The bundle version is stamped from `workspace.package.version` at build time (no more hardcoded plist version). First launch from a download still needs one `xattr -cr /Applications/Helion.app` (documented in the README) until Apple notarization is set up.
- UX audit fixes: red Timing warnings resolved, bottom panel holds its drag position, the Tcl tab no longer pops a floating dialog (inline console under the "Console" tab instead), toolbar scrolls on narrow windows, console opens with a clean "session started" separator.

[2.5.0]: https://github.com/helion-fpga/helion/compare/v2.1.0...v2.5.0

## [2.1.0] — 2026-10-11

Performance campaign, area campaign, and IDE rebuild.

### Added

- Permanent criterion benchmark harness for all five engines (synth, pack, place, route, STA), a full-flow profiler, and a perf-budget proposal.
- ABC-style AIG rewriting in synthesis: structural hashing, K=4 cut enumeration, exact minimum-AND truth-table resynthesis, and cross-cone structural sharing via one global hashed AIG — all guarded by exhaustive miter equivalence tests.
- IDE: rebuilt Project Summary (8 space-filling sections), real code editor (line-number gutter, syntax highlighting, breadcrumbs, diagnostic gutter ticks, CJK font fallback), expandable hierarchy tree, schematic level-of-detail (hierarchical block treemap at fit zoom, gate sheet on zoom-in), sortable timing tables with endpoint-slack histogram, clickable status bar.
- 80-surface screenshot gallery (`helion-research/surfaces/`) covering PicoRV32 and Ibex across all 28 workspace tabs.
- README overhaul with hero screenshots, measured benchmarks, and architecture/roadmap docs.

### Changed

- Synthesis frontend allocations 12.7M → 409k (−97%); frontend wall time −69% (arena-allocated elaboration, borrowed tokenizer spans, arena parser, interned names).
- Area: uart 420 → 244 LUTs (−42%), spi 73 → 52 (−29%), cnt16 37 → 16 (−57%), cmp16 39 → 20 (−49%); PicoRV32 −19% LUTs from AIG rewriting and −10% further from cross-cone sharing (63% of cross-cone logic was structurally duplicated).
- `Rtl`/`RExpr` now borrow `&str` name spans and arena-allocate nodes instead of per-node `String`/`Box`.

### Fixed

- Unpacked-word output IOBs were attached to undriven net names while the real output nets went nowhere.
- 16-bit counters silently dropped bit 15.
- Cross-cone sharing polarity corruption (an output node reused inside another cone's logic) — caught by new multi-cone miter tests before shipping.
- DRC message column clipped at 1600px; Timing Constraints button now shows the actual filename.
- Ibex CJK comments rendered as `?????` (bundled font lacked CJK glyphs; DroidSansFallback wired as fallback).

[2.1.0]: https://github.com/helion-fpga/helion/compare/v2.0.5...v2.1.0

## [2.0.5] — 2026-10-01

School-schedule catch-up release with critical PNR, DRC, GUI, and SV fixes.
*Note from maintainer: Updates have been slightly delayed recently due to demanding school commitments.*

### Changed

- Scaled Pathfinder A* heuristic by `HOP_DELAY_PS` (40 ps) in `helion-route`, preventing search degradation into unguided Dijkstra exploration.
- Pre-indexed driver nets into a `HashSet` in `helion-drc` for `ROUTE-3`, reducing check complexity from O(I×N) to O(I+N).
- Added 50ms polling backoff to GUI background task receiver (`TryRecvError::Empty`), eliminating 100% CPU core busy-spinning during CAD runs.
- Historical baseline: 9b864af all 272485 B dense bitstream.

### Fixed

- Fixed dead stores in `crates/helion-route/src/lib.rs` (`overused`, `last_paths`).
- Restored missing `#[test]` attributes on 3 dead tests in `crates/helion-sv/src/lib.rs` (`bufif1_mux_is_one_gate_primitive_not_a_lut`, `clock_mux_posedge_is_not_a_user_clock`, `string_param_width_does_not_panic`).
- Resolved QoR test assertions in `crates/helion-cli/tests/qor.rs` by adding semantic markdown table column parsing.
- Fixed unused mutability and deprecated `SelectableLabel` compiler warnings across `helion-sv`, `helion-gui`, `helion-vhdl`, and `helion-lab`.

[2.0.5]: https://github.com/helion-fpga/helion/compare/v2.0.4...v2.0.5
## [2.0.4] — 2026-09-16

OPT P1-2 STA HashSet (#62), P1-3 bitgen frame buffer (#63), place-legalize
(#64), residual synth_rtl (#65), residual parse (#66). Empty-XDC
`examples/counter.sv` gold unchanged (**WNS_PS=9640**). SOFT ≠ PASS held.
Release `helion impl examples/ibex_pin_wrap.sv`: parse **416→353 ms**,
synth_rtl **670→524 ms**, place legalize **213→23 ms**, imux_skip **158→123**.
Native Apple Silicon `Helion.app` ships on this GitHub Release.

### Changed

- `report_timing_placed` uses a HashSet of IOB `from_net` (O(1) membership;
  kills O(lutffs×iobs)).
- Bitgen writes through a frame buffer (no `.hbits` format growth).
- Place legalize: dense occupancy grid, remaining-illegal worklist, pin-index
  scoring, high-fanout driver sampling.
- `synth_rtl`: single-pass `emit_module_into`; process-level OwnCache; no Rtl
  clone before synth.
- Parse: process-level `SyntaxTree` Arc cache (`OnceLock+Mutex`).

### Fixed

- Ibex pin-wrap wall after #64/#65/#66: legalize −89%, synth_rtl −22%,
  parse −15% vs post-#64 / post-#65 baselines. Diagnostic depth unchanged.

[2.0.4]: https://github.com/helion-fpga/helion/releases/tag/v2.0.4

## [2.0.3] — 2026-09-16

OPT P0-2 stitch/cache + P0-3 parse preprocess (`wip/opt-sv-wall` / #60). Empty-XDC
`examples/counter.sv` gold unchanged (**WNS_PS=9640**). SOFT ≠ PASS held. On
release `helion impl examples/ibex_pin_wrap.sv` after rebase: parse ~**347 ms**,
synth_rtl ~**644 ms** (cells=16756 luts=11572). Native Apple Silicon `Helion.app`
ships on this GitHub Release.

### Changed

- `OwnCache` holds `Arc<Design>`; hierarchy stitch appends cells with a port
  HashMap (no 13k-cell clone per wrapper); assign_cap / early LUT6 before
  `expr_node_count`. No AIG/flowmap rewrite.
- Parse: byte preprocess (no `Vec<char>`), leftover-backtick pass; path+mtime
  `SyntaxTree` cache (thread-local `Arc`).

### Fixed

- Ibex pin-wrap wall: synth_rtl first-run cut ~16% vs pre-opt scout pair;
  parse ms down. Diagnostic depth unchanged.

[2.0.3]: https://github.com/helion-fpga/helion/releases/tag/v2.0.3

## [2.0.2] — 2026-09-15

Learner L1–L4 surfaces + OPT P2-1 / P0-1 / P1-1. Empty-XDC `examples/counter.sv`
gold matches `helion qor`: **WNS_PS=9640**, LUTFF=4, IOB=1, BYTES=185. Native
Apple Silicon `Helion.app` ships on this GitHub Release.

### Added

- SoftDiag / MapResult IR schema (L3); VHDL emit into `MapResult.softs` (no
  parallel table). SOFT ≠ PASS.
- L2 HAD stable Site/BEL/PIP/Net IDs; PNR highlight set + GUI HighlightSet /
  packing wire-up (L2-UI).
- L1 GUI status rail provenance; L4 overlay `sim_DONE` + stable
  `HELION_E_EMPTY_BITSTREAM` / `HELION_E_USB_0` refuse codes.

### Changed

- Cap/QA scripts prefer release `helion` when present (OPT P2-1).

### Fixed

- Budgeted IMUX legalize under affinity_cap (OPT P0-1).
- PathFinder A* buffer reuse, denser congestion, IOB→lutff index (OPT P1-1).
- Multi-IOB path delay / highlight indexing in STA/route (L2-PNR).

[2.0.2]: https://github.com/helion-fpga/helion/releases/tag/v2.0.2

## [2.0.1] — 2026-09-15

Soft-clear polish on the 2.0.0 bar. Empty-XDC `examples/counter.sv` gold is
unchanged (**WNS_PS=9640**). Native Apple Silicon `Helion.app` ships on this
GitHub Release.

### Fixed

- `generate_not_lowered` prim_ram_1p_scr; hierarchical outstanding_*/RVFI
  assigns. cells 17027→16741, luts 11845→11559. fcov_ not invented.

[2.0.1]: https://github.com/helion-fpga/helion/releases/tag/v2.0.1

## [2.0.0] — 2026-09-15

Public OSS CAD bar. A stranger can clone, `cargo test --workspace` (helion-gui
lib green), run headless gold **WNS_PS=9640**, and trust the legal fence.
Native Apple Silicon `Helion.app` ships on this GitHub Release.

### Added

- Disk `.hckp` CLI ECO path (`read_checkpoint` alias; open → ECO → bitstream
  hash change).
- IDE clock/led-optional contract (VHDL keep-name; HFF add_wave).
- Soft-clear track: Ibex wide_cone / if_stage generate / assign batches /
  rvfi+icache generate lowering with named leftovers (SOFT ≠ PASS).

### Fixed

- helion-gui lib: 189 passed (was 14 failing on timing panes, UG900, Zoom Fit,
  scopes, expand_inside).
- Lab honesty: empty refuse; no-cable refuse DONE; overlay labeled.
- PNR issue #16 closed (timing_weight moves closed WNS).

### Legal fence (unchanged)

Original Helion ISA/CAD only. No Project X-Ray, UNISIM, vendor Tcl product
names, or AMD/Intel/Lattice backends. Helion-MM / Helion-ST only.

[2.0.0]: https://github.com/helion-fpga/helion/releases/tag/v2.0.0

## [1.6.0] — 2026-09-15

IDE clock/led contract + disk `.hckp` CLI ECO. Empty-XDC `examples/counter.sv`
gold is unchanged (**WNS_PS=9640**). Native Apple Silicon `Helion.app` ships on
this GitHub Release.

### Added

- CLI `open.hckp` → ECO LUT → `write_bitstream` hash change; `read_checkpoint`
  alias for restore.
- VHDL clock port keep-name so GUI HFF / add_wave contract holds on rng/blinky.

### Fixed

- Lab honesty already on master (empty refuse / no-cable refuse DONE / overlay
  labeled). Issue #16 PNR closed-WNS closed.

[1.6.0]: https://github.com/helion-fpga/helion/releases/tag/v1.6.0

## [1.5.4] — 2026-09-15

Soft-clear: `rvfi_ext_mip` + remaining `generate_not_lowered` (icache/PMP
package sizes, typed arrays, packed structs, genvar, int'()). Empty-XDC
`examples/counter.sv` gold is unchanged (**WNS_PS=9640**). Native Apple Silicon
`Helion.app` ships on this GitHub Release.

### Fixed

- Broader generate/assign lowering for Ibex pin-wrap (cells→17027, luts→11845
  on tip; remaining softs stay named diagnostics, not silent).

[1.5.4]: https://github.com/helion-fpga/helion/releases/tag/v1.5.4

## [1.5.3] — 2026-09-15

Soft-clear: Ibex `assign_not_lowered` batch (ALU / CSR / counter / timer /
core_busy). Empty-XDC `examples/counter.sv` gold is unchanged (**WNS_PS=9640**).
Native Apple Silicon `Helion.app` ships on this GitHub Release.

### Fixed

- assign_not_lowered: bwlogic_or/and, illegal_csr_*/csr_rdata_int/mie_d,
  counter_upd, core_busy_o, mtime_inc/interrupt_d. cells 14484→14896,
  luts 9340→9979.

[1.5.3]: https://github.com/helion-fpga/helion/releases/tag/v1.5.3

## [1.5.2] — 2026-09-15

Soft-clear: `ibex_if_stage` `generate_not_lowered` maps packed-struct field
selects. Empty-XDC `examples/counter.sv` gold is unchanged (**WNS_PS=9640**).
Native Apple Silicon `Helion.app` ships on this GitHub Release.

### Fixed

- `generate_not_lowered` **ibex_if_stage** cleared (child_soft_incomplete cascade
  gone). cells 13559→14484, luts 8491→9340.

[1.5.2]: https://github.com/helion-fpga/helion/releases/tag/v1.5.2

## [1.5.1] — 2026-09-15

Soft-clear: named Ibex `wide_cone` softs map to real LUT/FF. Empty-XDC
`examples/counter.sv` gold is unchanged (**WNS_PS=9640**). Native Apple Silicon
`Helion.app` ships on this GitHub Release.

### Fixed

- `csr_pipe_flush` / `op_remainder_d` / `r_state` wide_cone paths lower with
  OR-of-eq LUT trees, packed bit mux, and sequential ITE (no hang-cap raise).
  Ibex pin_wrap LUTs ~8073→8491 (honest map).

[1.5.1]: https://github.com/helion-fpga/helion/releases/tag/v1.5.1

## [1.5.0] — 2026-09-15

FM-HEL-14-W1b mapping density. Empty-XDC `examples/counter.sv` gold is
unchanged (**WNS_PS=9640**). Native Apple Silicon `Helion.app` ships on this
GitHub Release.

### Added

- Honest LUT6 packing for ≤6-PI mux/case, shared identical (INIT, pins) across
  bus bits, and denser XNOR/AND INIT absorption on wide cones.

### Changed

- `ibex_pin_wrap` LUT count **108545 → 8073** (fits affinity SKU 8192);
  cells 113611→13139. Ibex closed WNS **8100** held (not invented).

[1.5.0]: https://github.com/helion-fpga/helion/releases/tag/v1.5.0

## [1.4.0] — 2026-09-15

FM-HEL-14 milestone 1.4: SystemVerilog elaborator depth on the 1.3.1 base.
Empty-XDC `examples/counter.sv` gold is unchanged (**WNS_PS=9640**). Native
Apple Silicon `Helion.app` ships on this GitHub Release.

### Added

- Called-function / `$readmemh` → Bram18 INIT / `(* ram_style="block")` forces
  BRAM; generate/case/always lowering that previously SOFT now maps to real
  LUT/FF or a named diagnostic (once-per-design; never silent drop).
- Unit coverage in `helion-sv` for the new inference paths.

### Changed

- Public 1.4 track closed for the elaborator lock; 1.5 density (W1b) starts
  after this tag.

### Fixed

- Soft-incomplete / function-not-called hang classes finish with a diagnostic
  instead of walking until kill on the covered cases.

[1.4.0]: https://github.com/helion-fpga/helion/releases/tag/v1.4.0

## [1.3.1] — 2026-09-15

FM-HEL-14 parallel landings after the 1.4 baseline. Empty-XDC
`examples/counter.sv` gold is unchanged (**WNS_PS=9640**). Native Apple Silicon
`Helion.app` ships on this GitHub Release.

### Added

- [`ROADMAP.md`](ROADMAP.md) for 1.4 → 1.5 → 1.6 → 1.7 → 2.0, linked from README
  and GitHub Pages ([roadmap.html](https://helion-fpga.github.io/helion/roadmap.html)).
  Open milestones
  [1.4](https://github.com/helion-fpga/helion/milestone/1) /
  [1.5](https://github.com/helion-fpga/helion/milestone/2) /
  [1.6](https://github.com/helion-fpga/helion/milestone/3) /
  [1.7](https://github.com/helion-fpga/helion/milestone/4) /
  [2.0](https://github.com/helion-fpga/helion/milestone/5).
- HAD load-only parts `HL10M-C128-1` / `HL10S-C64-1` and board `HB1`.
- Disk `.hckp` checkpoints with strategy / pblock / placement restore; `.prj`
  keeps pblock + impl run; pblock cell lists and XDC honored on project paths.
- VHDL elaborator path: real entity bodies, `clock≡clk` for STA (narrow), named
  `missing_component` diagnostics (no silent `cells=0`).
- Timing-driven P&R: `timing_weight` / false-path / multicycle move closed WNS
  on the hard fixture (not report text only).
- Lab honesty: refuse empty bitstream; overlay counter LED labeled overlay;
  native/OFL refuse DONE without cable / empty frames.
- IDE name-contract: surface Open errors; HFF CLK for wave/bits; O(visible)
  hierarchy drawings; `examples/rng.vhd` / `adder4.vhd` open path.

### Changed

- CI: Ubuntu and `macos-latest` require headless gold **WNS_PS=9640**; macos
  release `helion-ide` check parity with ubuntu.
- Public Pages / llms.txt / architecture docs point at roadmap and `load_xdc`
  command list only.

### Fixed

- Pblock occupancy legalization (no duplicate site/ble); IOB checks after LOC
  reorder; bad pblock range fails closed.

[1.3.1]: https://github.com/helion-fpga/helion/releases/tag/v1.3.1

## [1.3.0] — 2026-09-10

Pin-wrap suite, deep ILA, CDC methodology, Helion-ST/MM catalog, and hang-class
lowering on the 1.2.0 elaborator. Empty-XDC `examples/counter.sv` gold is
unchanged (**WNS_PS=9640**, `.hbits` 185 B). Public corpus waves are hang-free
and do not close WNS on `cells=0`; they are **not** 100% PASS (mostly SOFT).

### Added

- Ibex / PicoRV32 / SERV pin-wrap examples with user SDC and heartbeat WNS.
- Mark Debug → Add Probe → Program sim → ILA Arm. Deep ILA uses fabric BRAM
  capture, IR_USR1 scan, match-unit / trigger FSM (`ip/h_ila_trig`), and RLE
  upload. Soft LUTFF probe remains; this is not a vendor ILA clone.
- CDC-1/10/13/14/15/16, TIMING-10, POWER-2 in Reports catalog (Failed / Warnings
  / Info, not empty-green Complete).
- Helion-ST and Helion-MM catalog cores with real fabric (plus Helion-DBG
  `h_ila_trig`). Never AXI.
- Create Project / Recent `.prj` / `save_project_as` / `close_project`, editable
  Constraints SDC, pblocks, Simulate wave, Device STA-path highlight.

### Fixed

- Bitgen refuses empty/fake `.hbits`. `write_bitstream` / board program refuse
  DONE without a cable. `helion reports` still matches occupancy on 0-LUT
  wrappers.
- Hang-class Verilog (logikbench arbiter / sha256_k / bin2prio, Ibex ITE density)
  lowers or diagnoses; does not walk until kill.
- LUT6 ROM collapse is gated to case-of-const.

[1.3.0]: https://github.com/helion-fpga/helion/releases/tag/v1.3.0

## [1.2.0] — 2026-09-09

Behavioral Verilog lowering on the 1.1.1 elaborator. `assign`, `always`/`case`,
and functions (called, or lifted into an otherwise empty module) map to Helion
LUT/FF. Ports-only shells and unknown vendor instances with no body in the file
stay `cells=0` / `no_body`: no invented gates, no vendor IP clone, no fake closed
WNS. Empty-XDC `examples/counter.sv` gold is unchanged (**WNS_PS=9640**).

### Added

- Standing elaborator rule: behavioral Verilog that has a body lowers to LUT/FF.
  Not filename-special-cased.
- Bounded clocked shifts, indexed concat assigns, constant range compares,
  constant word aliases, and a 16-bit / named-bus add onto Hffs or a ripple of
  full adders.
- `examples/primitives/helion_cells.v` — named HELIONLIB cells for the same path.

### Fixed

- Empty shells quote `synth_design … cells=0 luts=0 no_body`. Timing is that
  diagnostic, not a closed `WNS_PS`.
- WNS closes only on a real clock path. Muxed, gated, and leftover `clk` names
  do not pretend to be one user clock.
- A clocked `always` that does not lower to an Hff fails loudly.
- Skipped wide cones report `timing_incomplete` (one diagnostic), not a
  `node_count` storm or a fake `no_clock_path`.
- Sized-binary left-shift and range-width add overflow are diagnostics, not panics.
- Ibex synth finishes after one `function_not_called` instead of flattening the tree.
- Place names the LUTFF legalize cap instead of dying at 783 LUTFFs.
- Device / schematic panes keep leftover height and honest timing on narrow windows.

[1.2.0]: https://github.com/helion-fpga/helion/releases/tag/v1.2.0

## [1.1.1] — 2026-09-08

Patch: the IDE Open / Synth rail actually elaborates `.vhd` / `.vhdl`.
1.1.0 CLI already used `helion-vhdl`; the desktop app still called
`synth_sv_path`, so `examples/blinky.vhd` failed in Helion.app. Gold
empty-XDC `counter.sv` is unchanged (**WNS_PS=9640**).

### Fixed

- IDE `open_source` / `synth_design` / incremental impl dispatch `.vhd` and
  `.vhdl` through `helion-vhdl` (`synth_hdl_path`). `blinky.vhd` maps LUT+FF
  and paints the schematic.

[1.1.1]: https://github.com/helion-fpga/helion/releases/tag/v1.1.1

## [1.1.0] — 2026-09-07

Language coverage and schematic camera. Same family, same empty-XDC
`examples/counter.sv` gold (**WNS_PS=9640**, LUTFF=4, LED `0000000111111110`).
`helion reports` is one compile: timing, utilization, power, and bitstream
occupancy agree (`match=LUTFF,IOB,BRAM,DSP,TOTAL_UW`).

### Added

- VHDL-2008 elaborator maps concurrent and sequential constructs to the SV
  LUT/FF path: `with`/`select`, `when`/`else`, generate, process
  if/elsif/case/for, generics, multi-name ports, `clk'event`, concatenations,
  IEEE casts, component port maps. Tokenize never fails on a leftover glyph.
- SystemVerilog `||` / `&&`, concat LHS bit-selects
  (`{out[0],out[1],...}=in`), and wire-through `assign out = in` (IOB, not a
  dropped net).
- `helion reports <src>`: one compile prints timing + util + power + bitstream
  and exits 0 only when occupancy and `TOTAL_UW = STATIC+DYNAMIC` match.
- Schematic camera: pinch / ⌘-scroll zoom-at-cursor, drag pan, Zoom Fit.
  Large sheets are not auto-shrunk. Star net edges, `Arc<SchematicDrawing>`
  cache so Ibex-scale sheets stay idle.

### Fixed

- Place caps LUTFF/IOB/DSP/BRAM to HAD site counts without panicking at the
  8192 BLE budget.
- PathFinder overused tiles and undriven IOBs are DRC warnings; bitstream
  still builds. Route skips an IMUX whose driver FF was truncated to the die.
- Identity and empty-architecture designs produce matched reports (honest
  0-LUT wrappers), not `nothing to pack`.

### Changed

- `helion-vhdl` is an elaborator, not a keyword skip-list. Missing child
  architectures become empty stubs (missing source), not unknown syntax.

1.1.0 was measured on a 1000-file Verilog+VHDL corpus (NVlabs/verilog-eval,
SJTU CORE, YosysHQ/yosys tests, hdl2v/vhdl-dataset): **1000/1000** `helion
reports` match on HL10T-C32-1.

[1.1.0]: https://github.com/helion-fpga/helion/releases/tag/v1.1.0

## [1.0.1] — 2026-09-07

Patch after 1.0.0: IDE chrome and empty-state fill. Same CAD engines and
`counter.sv` gold (**WNS_PS=9640**). No family or bitstream format change.

### Fixed

- Activity rail shows full names (Files / Device / Timing / Simulate / Program / Reports).
- Schematic opens in the main pane; unconnected pins autohide unless the cell is selected.
- Device die, Package pins, Wave traces, Program ILA, and occupancy tables fill the pane instead of leaving dead gaps.
- Toolbar Open / flow / Bitstream / Implement share one control height.
- Simulate no longer leaves the Timing tab selected; Reports catalog stays in the sidebar.
- Native Open HDL stays real on macOS and Linux (rfd pinned for rustc 1.85).
- Program no longer shells `openFPGALoader` on every paint frame.
- Release `helion-ide` compiles: hover-debug is debug-only (`egui` `set_debug_on_hover`).

### Changed

- Empty Bitstream / Find / DRC / Power / Utilization / Methodology canvases use a remaining-pane next action.

[1.0.1]: https://github.com/helion-fpga/helion/releases/tag/v1.0.1

## [1.0.0] — 2026-09-06

First public suite: original Helion family + in-repo CAD on native
`aarch64-apple-darwin`, gated on Linux CI.

### Added

- HAD part **HL10T-C32-1** (32×32 CLB, IO ring, clock spine, idcode `0x00011A1F`)
  and HELIONLIB primitives LUT6, HFF, MAC27, BRAM18. Device facts live in HAD,
  not hardcoded in the CAD.
- Flow: SystemVerilog (`sv-parser` + AIG + FlowMap LUT6), VHDL-2008 subset,
  C subset HLS, pack, place, PathFinder route, graph STA, DRC, FeatureMap
  `.hbits`, cycle-accurate fabric sim.
- Tcl Session (`synth_design`, `place_design`, `route_design`, `write_bitstream`,
  `mark_debug`, `report_timing`, eco, checkpoints) calling those engines.
- Desktop IDE (`Helion.app` / `helion-ide`) and CLI (`helion`).
- Empty-XDC `examples/counter.sv` gold: **WNS_PS=9640**,
  `LED[16]=0000000111111110` (LED = cnt[3]).
- FM-HEL corpus: 100 / 100 PASS on the published set.

### Honest limits

- No live FTDI on the gated Linux host: detect stays 0 probes; program DONE is
  not claimed without hardware.
- ILA is a `mark_debug` LUTFF probe with fabric BLE-Q readback, not on-chip
  capture RAM / JTAG upload.
- Default C32-1 grid has `mac27=0`; DSP pack is demonstrated on HL10T-DSP1.
- macOS builds are **unsigned**. Gatekeeper will warn until notarization exists.
- Legal fence: no Project X-Ray, UNISIM, vendor Tcl, or AMD/Intel/Lattice
  bitstream backends.

[1.0.0]: https://github.com/helion-fpga/helion/releases/tag/v1.0.0
