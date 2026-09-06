# FM-HEL-UX2 — CAPTAIN intensive IDE UI/UX

**Date:** 2026-09-06 ~07:27 America/New_York (EDT)  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Score tip (branch HEAD):**  ()  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`

## Gold

```
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

## Layout fix (captain: empty space / ugly Reports)

Reports/Timing no longer use a SidePanel that left a ~500px black void beside the canvas.
Canvas-native split: navigator (Reports catalog / Timing paths) | detail (Timing Summary + paths).
More ⋯ always lists overflow WorkspaceTab destinations (zero deletion).
Sidebar width capped (default 220 / max 280) for Files/Device/Program.

## Shots

Under `/workspace/fm-hel-ux2/shots/`:

| Path | Content |
|------|---------|
| `01`–`13` | prior pack (launch / device / rail / toolbar) |
| `20-timing.png` | Timing canvas — WNS table / paths, no void |
| `30-sim-wave.png` | Sim + Wave English empties + CTAs |
| `40-more.png` | More ⋯ open — Schematic, Wave, Package, Runs, … |
| `50-reports-runs.png` | Reports rail view — catalog + timing detail, void gone |
| `50-reports-or-runs.png` | alias of 50 |
| `51-reports-no-void.png` | proof alias after layout fix |

## Remains

Helion UX HARD rescore on this tip + full pack. **NO MERGE** until captain/Firstmate.

## Push

`helion-fpga` `fm-hel-corpus-soft-pass` tip  — no merge, no force.
