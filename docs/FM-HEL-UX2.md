# FM-HEL-UX2 — CAPTAIN intensive IDE UI/UX

**Date:** 2026-09-06 ~06:57 America/New_York (EDT)  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Score tip (branch HEAD):** `fbdd9e02ce8b688aa46bb4e229b1aa56be5f0617` (`fbdd9e0`)
**Chrome+hooks tip:** `a61ce83` (ancestor of HEAD)  
**Chrome commit:** `5d20f16` (intensive betterment); follow-ups `5193ff4` report, `2e1ec6a`/`81adc9c` macOS `Command` import, `a61ce83` HELION_OPEN/FLOW hooks  
**Remote:** `helion-fpga` PR #7 branch `fm-hel-corpus-soft-pass`  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`

## Gold

```
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

Native status bar after Implement (shot 02): `HL10T-C32-1 · WNS 9640 · LUTFF 4/8192 · impl_1`.  
Air: no idle `request_repaint`; calm splitter grab `SPLITTER_GRAB_PX=6`.

## Shots (Mac native helion-ide)

Under `/workspace/fm-hel-ux2/shots/` (also Mac `/Users/saksham/helion/fm-hel-ux2-shots/`):

| Path | Content |
|------|---------|
| `01-main-empty.png` | First launch — Open/Recent/Examples, 3 canvases, rail names |
| `02-counter-device.png` | counter.sv + Implement — Device canvas, die, Console ok lines |
| `03-device-floorplan.png` | same as 02 (die/floorplan alias) |
| `04-counter-editor.png` | counter open on Editor |
| `10-toolbar-strip.png` | Open… / flow strip / Bitstream + Implement |
| `11-activity-rail.png` | F Files … P Program … R Reports + Console |
| `12-toolbar-after-impl.png` | green Synth–Route chips after Implement |
| `13-device-canvas.png` | cropped die/canvas |

Window-id `screencapture -l` denied (Screen Recording); used `-R` window bounds. GUI process name `helion-ide`, title Helion.

## What improved

- MUST 1–12 **held**; cycle2b wins held (3 canvases+More, counter synth cells=9/luts=4, Timing≠Reports, English pblocks, Open/Recent/Examples, Bitstream secondary, Wave ns period, zero deletion).
- Empty English + one CTA (clocks, DRC, timing, ECO, incremental, waveform Run Simulation, Program Generate Bitstream, Place/Route/Synth, netlist Implement).
- Hits: `HIT_COMFORT=36` Implement/Open; progress 32px; IO/Wave chips ≥28; bottom tabs 28px.
- Rail: short **names** emphasized; `Program` full short label.
- Die: `DEVICE_TABLES_MAX_HEIGHT` 220→140.
- Occupancy bars 20px; calm 6px splitter grab; dump crumbs stripped (`sel=`, softer bitstream/schematic).
- `HELION_OPEN` / `HELION_FLOW=implement|synth` launch hooks (demos/shots; Open… unchanged).
- macOS `Command` import cfg-gated for Open dialog build.

## Remains for Helion UX HARD PASS

1. Captain re-score with these shots (die fill % / splitter Δ labeled overlays if needed for cycle3).  
2. Letter+name rail still letter-led (names present) — further iconography optional NICE.  
3. Some More panes still soft-English without CTA.  
4. **NO MERGE** until captain HARD PASS.

## Push

`helion-fpga` `fm-hel-corpus-soft-pass` tip **`a61ce83`** (no merge, no force).
