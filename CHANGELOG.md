# Changelog

All notable Helion releases are listed here. Version numbers match
`workspace.package.version` in `Cargo.toml` and Git tags `vMAJOR.MINOR.PATCH`.
Downloadable builds are on [GitHub Releases](https://github.com/helion-fpga/helion/releases).

## [Unreleased]

### Added

### Changed

### Fixed

[unreleased]: https://github.com/helion-fpga/helion/compare/v1.5.4...HEAD

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
