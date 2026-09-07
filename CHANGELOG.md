# Changelog

All notable Helion releases are listed here. Version numbers match
`workspace.package.version` in `Cargo.toml` and Git tags `vMAJOR.MINOR.PATCH`.
Downloadable builds are on [GitHub Releases](https://github.com/helion-fpga/helion/releases).

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
