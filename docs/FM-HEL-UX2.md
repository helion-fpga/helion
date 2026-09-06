# FM-HEL-UX2 — CAPTAIN intensive IDE UI/UX

**Date:** 2026-09-06 ~06:55 America/New_York (EDT)  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Tip SHA:** `5d20f1658e83c28259fce004c1405ffa2859856b` (`5d20f16`)  
**Parent tip:** `8e34bd2`  
**Remote:** `helion-fpga` `8e34bd2..5d20f16` (PR #7 branch)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`

## Verdict (executor)

MUST 1–12 **held** from prior chrome (e003216 / gap5 / cycle2b). Intensive betterment landed on tip without feature/button deletion. Native Mac shots **deferred** (Mac unreachable). Gold **held**.

## Gold

```
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
→ report_timing counter WNS_PS=9640 TNS_PS=0 endpoints=4
→ hang_diag route imux_skip=0
```

Air: no idle `request_repaint` (reactive eframe only; drag-gated splitters via `SPLITTER_GRAB_PX=6`).

## What improved (this commit)

| Area | Change |
|------|--------|
| Empty states (MUST 7 leftovers) | English + one CTA: constraints, timing paths, clocks, DRC, incremental, ECO, netlist, Place/Route/Synth, waveform Run Simulation, Program Generate Bitstream |
| Program / Debug | Detect + cable chips at ≥28px; Bitstream CTA when empty; result text English (`N frames · M B`), not `frames=` dump |
| Hits / density | `HIT_COMFORT=36` Implement/Open; progress chips 32px; IOSTANDARD/DRIVE/… chips ≥28; Wave Cursor/marker tools sized; bottom Console\|Messages tabs 28px |
| Rail names (MUST 3 secondary) | Short label emphasized (10px); `Program` full short name (7 chars); letter secondary |
| Die fill | `DEVICE_TABLES_MAX_HEIGHT` 220→140 so floorplan keeps canvas (cycle2b ≥80% bar intent) |
| Occupancy | Bars 12→20px (`OCCUPANCY_BAR_H`) |
| Splitters | Explicit calm `resize_grab_radius_side=6` (no neon QA slab) |
| Dump crumbs | Stripped console `sel=`; bitstream idcode line English; schematic cone/zoom English; DRC/hierarchy softer |
| Wave | Legend `click cursor · Shift A · Alt B`; empty “No waveform yet.” + **Run Simulation** (no `sim_run`/`add_wave_marker` crumb paint) |
| Toolbar | 44px row; Bitstream stays secondary/narrower vs Implement |

## MUST 1–12 (held)

1. Editor \| Device \| Timing + More — yes  
2. One Implement primary; Bitstream secondary — yes  
3. Activity rail icon+short name — yes (Program named)  
4. Open… + Recent; Examples menu — yes  
5. One sidebar + ⌘J collapse; tables capped for die — yes  
6. Hits ≥28–32 (comfort 36 primary) — yes  
7. English empty + CTA — improved this pass  
8. ⌘O / ⌘↩ / ⌘1–3 / ⌘P palette / ⌘J / ⌘\` — yes  
9. Status `Part · WNS · LUTFF · Run`; no `n=`/`engine=` paint — held  
10. More ⋯ not wrap — yes  
11. Bottom Console \| Messages (+ Sim log only in Simulate) — yes  
12. Constraints Add…; Runs Launch + New — yes  

## Cycle2b holds

3 canvases+More; Timing ≠ Reports dual; English pblocks path; Open/Recent/Examples; Bitstream narrow; Wave “N ns per cycle”; zero deletion (More ≤2 clicks). Splitters remain resizable (Δ≥40 when dragged — egui grab 6px calm).

## Shots

- Intent path: `/workspace/fm-hel-ux2/shots/`  
- **Honesty:** Mac `e6c67522-…` unreachable during ship — no native `helion-ide` screenshots. See `shots/README.txt`. Re-capture: main, rails, tool strips, Device/floorplan, counter open.

## Files

- `crates/helion-gui/src/chrome.rs`
- `crates/helion-gui/src/bin/helion-ide.rs`

## Verify

```
cargo test -p helion-gui --lib chrome::   # 3 passed
cargo build -p helion-gui --bins         # ok
helion report_timing … → WNS_PS=9640
```

## Remains for Helion UX HARD PASS

1. **Native shot set** on Mac (main / rail / toolbar / Device die / counter) proving die fill ≥80%, splitter Δ, Wave English, Program CTA — without shots, captain cannot re-score UX2 HARD PASS.  
2. Soft-English panes still without one-click CTA in a few More views (acceptable).  
3. Linux host has no native Open dialog (macOS-only `osascript`) — expected; Mac is the UX gate.  
4. Do **not** merge; captain holds HARD PASS call after shots.

## Push

`git push helion-fpga HEAD:fm-hel-corpus-soft-pass` → `8e34bd2..5d20f16` (no merge, no force).
