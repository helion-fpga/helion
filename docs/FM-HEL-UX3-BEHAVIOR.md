# FM-HEL-UX3 behavior notes (MUST FIX floor)

## Root causes
1. **Schematic unreachable:** `set_canvas(Timing)` clobbered `WorkspaceTab::Schematic` → `Reports`. More → Schematic never painted `paint_schematic`.
2. **Rail→content black gap:** Reports/Timing skipped SidePanel (`{}`), leaving a dead slab feel; restored exact `sidebar_v4_*` panels with catalog/paths (no resizable void).
3. **Tabs clipped:** strip used `Editor  ⌘1` style; now label-only + horizontal ScrollArea; shortcuts on hover.
4. **Reports names truncated:** 4-col Grid in narrow width; now name-first vertical list with full titles.
5. **Tooltip in gap:** `on_hover_text` floated; now `Tooltip` aligned `RectAlign::RIGHT` of rail icon.

## Verify
- Gold: `helion-ide --headless examples/counter.sv` → `WNS_PS=9640`
- More → Schematic shows diagram (workspace preserved)
- Reports rail: sidebar catalog full names; canvas = detail only
- Timing rail: sidebar paths; canvas = summary (no Reports twin)
- Sim absolute 220|6|Wave untouched
