# FM-HEL-UX2 — Sim void fix (62) + tip reconcile

**Date:** 2026-09-06 ~07:57 America/New_York (EDT)  
**Branch:** `fm-hel-corpus-soft-pass` (PR #7, **NO MERGE**)  
**Score tip (branch HEAD):** `e521e452ec38cf2f13f3d3e7058d99468d540bf6` (`e521e45`)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`

## Gold

```
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

## Status (void class)

| Shot | Intent | Notes |
|------|--------|-------|
| 60 | Timing-only | Prior PASS (Firstmate / Helion UX) |
| 61 | Reports rail | Prior PASS |
| **62** | Sim scopes\|Wave ≤6px abut | **Re-shot** after absolute-rect split (no SidePanel, no horizontal wrap). Proof: `/workspace/fm-hel-ux2/shots/62-sim-calm.png` |
| 63 | More ⋯ overflow | Re-shot if strip shifted: `63-more-overflow.png` |

## Sim fix (62)

- `Activity::Simulate` sets `WorkspaceTab::Wave`
- No `SidePanel` for Sim (was leaving a thick void beside Wave)
- Canvas uses **absolute rects**: 220px scopes nav | `SPLITTER_GRAB_PX` (6) rule | Wave fills remainder via `scope_builder`/`UiBuilder::max_rect`
- Empty Wave paints a filled pane (no sparse header + black slab)

## Tip reconcile

Single tip SHA is branch HEAD `e521e45` (full `e521e452ec38cf2f13f3d3e7058d99468d540bf6`). Do not cite `63ec405` as tip — that was the prior chrome commit; HEAD moved.

## Merge

**NO MERGE** until Firstmate / Helion UX HARD PASS on 62.
