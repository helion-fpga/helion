# FM-HEL-UX2 — void-class fix + 60-series after-shots

**Date:** 2026-09-06 ~07:46 America/New_York (EDT)  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Score tip (branch HEAD):** `TIP_SHA` (`TIP_SHORT`)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`

## Gold

```
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

## Void-class fix

- Timing/Reports: **no SidePanel** (SidePanel left a black hole beside the canvas).
- Timing rail / ⌘3: **Timing Summary + paths only** — never Reports catalog twin.
- Reports rail: **Reports tab** + catalog; Report detail **collapsed** by default (no Timing Summary stacked under a void).
- Sim: scopes panel capped (max 260) — calm splitter.
- More ⋯ overflow list **held** (Schematic/Wave/Package/Runs/…).

## After-shots (proof)

| Shot | Path | Intent |
|------|------|--------|
| 60 | `/workspace/fm-hel-ux2/shots/60-timing-only.png` | Timing only — no Reports catalog, no pane void |
| 61 | `61-reports-rail.png` | Reports rail/catalog — no Timing twin, detail collapsed |
| 62 | `62-sim-calm.png` | Sim scopes\|Wave calm splitter |
| 63 | `63-more-overflow.png` | More ⋯ overflow list present |

## Note

Unused right margin on left-aligned tables may remain (compact grids) — that is not the pane-gap / Reports\|void\|Timing auto-fail class.

## Merge

**NO MERGE** until Firstmate / Helion UX HARD PASS void class.
