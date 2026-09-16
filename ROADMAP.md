# Helion roadmap (1.4 → 2.0 shipped; 2.0.x grind)

FM-HEL-14. Original FPGA family + in-repo CAD. Not a vendor-tool wrapper.

**Gold lock:** empty-XDC `examples/counter.sv` stays **WNS_PS=9640** (4 LUTFF, 1 IOB, LED `0000000111111110`) on every step. A WNS change must update the QoR table in the same commit with a reason.

Canonical markdown is this file. GitHub Pages: [docs/roadmap.html](https://helion-fpga.github.io/helion/roadmap.html).
Milestones: [helion-fpga/helion](https://github.com/helion-fpga/helion/milestones).
Current release: [v2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4).

| Release | Status | Workers | What | Milestone |
|---|---|---|---|---|
| **1.4** | shipped | W1 | SystemVerilog elaborator; gold 9640 | [1.4](https://github.com/helion-fpga/helion/milestone/1) |
| **1.5** | shipped | W2 + W4 | PNR/STA + project/CLI | [1.5](https://github.com/helion-fpga/helion/milestone/2) |
| **1.6** | shipped | W5 + W3 | IDE + lab | [1.6](https://github.com/helion-fpga/helion/milestone/3) |
| **1.7** | shipped | W6 + W8 | VHDL + HAD | [1.7](https://github.com/helion-fpga/helion/milestone/4) |
| **2.0** | shipped | — | Public OSS CAD bar | [2.0](https://github.com/helion-fpga/helion/milestone/5) |
| **2.0.x** | current | — | Post-bar STA / bitgen / place-legalize / synth / parse opt ships; gold lock unchanged | [v2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4) |

CI (W7) is not a product release. It gates gold on Ubuntu and `macos-latest` (both require `WNS_PS=9640`).

## 1.4 — SV elaborator (W1)

Lower public-widen SOFT: `always` / `case` / functions / memories / generates onto Helion LUT/FF. Infer BRAM18 / MAC27 from `$readmemh`, `ram_style`, and `a*b+c` when the body is real. Ports-only shells stay `cells=0` / `no_body` (no invented gates, no closed WNS). SOFT ≠ PASS.

Empty-XDC `examples/counter.sv` gold **WNS_PS=9640** is the ship gate.

[Milestone 1.4](https://github.com/helion-fpga/helion/milestone/1)

## 1.5 — PNR/STA (W2) + project/CLI (W4)

Timing-driven place/route must move **closed** WNS (sites/hops), not only TimingResult text. `set_false_path` / `set_multicycle_path` and `timing_weight` feed pack/place/route. Constraint commands Helion parses live in `crates/helion-sta/src/lib.rs` (`load_xdc`).

Project/CLI: `.prj` survives restart, disk `.hckp`, incremental, ECO that still bitgens. Tcl Session CAD (`synth_design`, `place_design`, …) stays a separate list from `load_xdc`.

[Milestone 1.5](https://github.com/helion-fpga/helion/milestone/2)

## 1.6 — IDE (W5) + lab (W3)

Desktop `helion-ide` chrome without mixing IDE hang work into elaborator commits. Three canvases (Editor / Device / Timing), activity rail, More overflow. CAD jobs stay on a `helion-engine` thread.

Lab: native FTDI / OFL STAT TDO **DONE=1**, then a real LED. No invented TAP STAT. No DONE without live TDO evidence.

[Milestone 1.6](https://github.com/helion-fpga/helion/milestone/3)

## 1.7 — VHDL (W6) + HAD (W8)

VHDL-2008 elaborator (`helion-vhdl`) on the same LUT/FF path as SV. Named diagnostics for missing components (not silent `cells=0`). No fake FD library.

HAD: Architecture Database parts beyond `HL10T-C32-1`. Device facts stay in TOML; CAD must query HAD, never hardcode a die.

[Milestone 1.7](https://github.com/helion-fpga/helion/milestone/4)

## 2.0 — public OSS CAD bar

**Shipped as [v2.0.0](https://github.com/helion-fpga/helion/releases/tag/v2.0.0).** A stranger can clone, `cargo test --workspace`, run headless gold **WNS_PS=9640**, and trust the legal fence: no Project X-Ray, no UNISIM, no vendor Tcl as product names, no AMD/Intel/Lattice backends, Helion-MM / Helion-ST only. Docs, CI (Linux + macOS gold), and the desktop IDE describe the same Session.

[Milestone 2.0](https://github.com/helion-fpga/helion/milestone/5)

## 2.0.x — post-bar runtime / QoR grind

**Current tip: [v2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4).** Patch line after the 2.0 bar: ship measured STA / bitgen / place-legalize / synth_rtl / parse cuts without moving gold. SOFT ≠ PASS held. Empty-XDC counter stays **WNS_PS=9640**.

Axes landed through 2.0.4 (see [CHANGELOG](CHANGELOG.md) and [docs/perf-2.0.4.md](docs/perf-2.0.4.md)):

- STA: `report_timing_placed` IOB `from_net` HashSet (O(1) membership).
- Bitgen: frame buffer write path (no `.hbits` format growth).
- Place legalize: dense occupancy grid, remaining-illegal worklist, pin-index scoring.
- `synth_rtl`: single-pass `emit_module_into`; process-level OwnCache.
- Parse: process-level `SyntaxTree` Arc cache.

Release `helion impl examples/ibex_pin_wrap.sv` deltas on the 2.0.4 notes: parse **416→353 ms**, synth_rtl **670→524 ms**, place legalize **213→23 ms**, imux_skip **158→123**.

Do not invent future marketing pillars here; next work is further measured grind on the same axes, or a new ROADMAP line when a real bar exists.

## Legal fence (every release)

- Original Helion ISA and CAD only.
- No Project X-Ray, UNISIM names, or vendor bitstream backends.
- No vendor Tcl trademarks as product names.
- No AXI interconnect as Helion IP.
- Gold **WNS_PS=9640** unless the QoR table moves in the same commit with a reason.
