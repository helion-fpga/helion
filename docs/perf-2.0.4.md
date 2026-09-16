# Helion 2.0.4 — ibex_pin_wrap stage deltas

Engineering table only. Source: [GitHub Release v2.0.4](https://github.com/helion-fpga/helion/releases/tag/v2.0.4) / [CHANGELOG](../CHANGELOG.md). Gold lock unchanged: empty-XDC `examples/counter.sv` **WNS_PS=9640**. SOFT ≠ PASS.

Command: release `helion impl examples/ibex_pin_wrap.sv`.

| Stage / axis | Before (ms) | After (ms) | Notes |
|---|---:|---:|---|
| parse | 416 | 353 | process-level `SyntaxTree` Arc cache (#66) |
| synth_rtl | 670 | 524 | single-pass `emit_module_into`; OwnCache (#65) |
| place legalize | 213 | 23 | occupancy grid + illegal worklist (#64) |
| imux_skip | 158 | 123 | count axis on same release notes |

Related: STA IOB `from_net` HashSet (#62); bitgen frame buffer (#63). No gold WNS move. No Soft→PASS.
