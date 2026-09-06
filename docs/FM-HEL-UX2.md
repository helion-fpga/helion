# FM-HEL-UX2 — Sim 62 abut fix (rescore)

**Date:** 2026-09-06 ~07:58 America/New_York (EDT)  
**Branch:** `fm-hel-corpus-soft-pass` (PR #7, **NO MERGE**)  
**Score tip (branch HEAD):**  ()  
**Sim abut chrome:** `e521e45` (parent of tip stamp)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`

## Gold

```
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

## Void class

| Shot | Path | Status |
|------|------|--------|
| 60 | `60-timing-only.png` | PASS (prior Firstmate/UX) |
| 61 | `61-reports-rail.png` | PASS (prior) |
| **62** | `/workspace/fm-hel-ux2/shots/62-sim-calm.png` | **Re-shot** — absolute 220\|6px\|Wave rects; pixel abut nav x287 → Wave x288 |
| 63 | `63-more-overflow.png` | Re-shot OK |

## Sim fix

- No Sim `SidePanel` (wrap void)
- `Activity::Simulate` → `WorkspaceTab::Wave`
- Absolute `scope_builder` rects: 220px nav | `SPLITTER_GRAB_PX` (6) | Wave fills
- Empty Wave fills its pane

## Merge

**NO MERGE** until Helion UX HARD PASS on 62.
