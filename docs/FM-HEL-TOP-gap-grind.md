# FM-HEL-TOP — gap grind (die-fill/splitter overlays + width-first floorplan)

**Date:** 2026-09-06 ~07:15 America/New_York (EDT)  
**Branch:** `fm-hel-corpus-soft-pass` (PR #7, **NO MERGE**)  
**Pick:** **D** (labeled die-fill/splitter overlays; paint fix so bar can clear) — A deferred (Mac USB still empty / Shell unreachable)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**Shipped SHA:** `ecc3706fec9df638d479603ea148aca97df4526a` (`ecc3706`)

## Why D over A

- UX2 remains asked for die-fill % / splitter Δ **labeled overlays** for captain cycle3.
- Measuring Mac shot `02-counter-device.png` showed honest **FAIL**: fill≈49.2%, right_gap≈710 logical px (cell max 24 letterboxed large panes).
- Labels alone would not help the UX score — shipped the **width-first** `floorplan_fit_cell` fix + unit tests (≥80% fill / ≤80px gap) so a Mac reshot can PASS.
- Live board / FTDI still soft-blocked (Mac listed connected, Shell temporarily unreachable; prior USB empty). No fake DONE.

## Gold

```
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

## What shipped

| Change | Where |
|--------|--------|
| `floorplan_fit_cell` width-first when letterbox &lt;80%; cell clamp 4..**64** | `crates/helion-gui/src/chrome.rs` |
| `floorplan_die_width` / `floorplan_die_fill_ratio` / `floorplan_right_gap_px` | same |
| Unit tests: ≥80% fill / ≤80px gap at 800×500 and Mac-like 1152×628 | `chrome.rs` + `ide.rs` |
| Labeled overlays (pre-fix evidence) | `/workspace/fm-hel-ux2/shots/20-device-fill-labeled.png`, `21-…`, `54-splitter-grab-labeled.png` |
| UX2 report remains / measurement table | `docs/FM-HEL-UX2.md` |

## Labeled overlay results (pre-fix Mac shots)

| Shot | fill | right_gap (retina / logical) | Verdict |
|------|------|------------------------------|---------|
| `20-device-fill-labeled.png` (from 02) | ≈49.2% | 1419 / ~710 | **FAIL** (documented) |
| `21-device-canvas-fill-labeled.png` (from 13) | ≈34.8% | 1419 / ~710 | **FAIL** (documented) |
| `54-splitter-grab-labeled.png` | grab=6px calm | — | labeled; Δ≥40 → cycle2b 54/55 held |

Post-fix math (unit-tested): 800×500 and 1152×628 → fill **100%**, gap **0**.

## Board / corpus / Air

| Item | Status |
|------|--------|
| Live FTDI / board DONE | **no** (Mac Shell unreachable this turn; USB previously empty) |
| Corpus | PASS 100 / SOFT 0 (unchanged) |
| Air idle | held (no `request_repaint`; calm splitter 6px) |
| Merge / force-push | **none** |

## Remains

1. Mac rebuild + Device reshot → captain re-score with new fill overlays.  
2. Optional: UX2 before/after splitter drag pairs when Mac local-exec works.  
3. Real FTDI/HAD for live OFL/`--cable native` STAT TDO (A track).  
4. **NO MERGE** until captain HARD PASS.

## Verdict

**PASS (bar moved).** Tip **`ecc3706`** pushed to `helion-fpga/fm-hel-corpus-soft-pass`. Die-fill root cause fixed + labeled measurement overlays for UX2; gold 9640 / skip=0 held; no board DONE claimed; no merge.
