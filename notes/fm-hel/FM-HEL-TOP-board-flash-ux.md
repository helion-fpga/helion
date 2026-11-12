# FM-HEL-TOP — Board flash UX (physical soft-hold)

**Date:** 2026-09-06 ~08:10 America/New_York (EDT)  
**Branch:** `fm-hel-top` (new PR off master after #7 merge, **NO MERGE**)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**Goal:** Honest Program-rail UX while Board A / FTDI is soft-hold. Do **not** claim board DONE.

## Constraints held

| Fence | Status |
|-------|--------|
| Board DONE claim | **none** — soft-hold banner + detect never invents a probe |
| Sim absolute split / `SPLITTER_GRAB_PX` / void-class Timing/Reports | **untouched** |
| Generate Bitstream / Program (sim) flows | **kept** |
| Gold `WNS_PS=9640` | **held** |
| Merge | **NO MERGE** |

## What shipped

| Change | Where |
|--------|--------|
| Clear soft-hold banner when `!physical_had`: *"Physical board soft-hold — no USB programmer detected. Use sim cable or attach FTDI/HAD."* | `paint_program_side`, `paint_hw` in `crates/helion-gui/src/bin/helion-ide.rs` |
| Detect → `program_status` scan summary `scan USB=N · OFL=… · physical_had=0\|1` (never fakes a probe); status heading **Scan** | `paint_program_side` |
| Guard Program when cable is `usb`/`native` (etc.) and `!physical_had` — disabled + honest hover; **sim / auto→sim still works** | `paint_program_side` |
| Hardware Manager soft-hold banner + last-scan line; sim **Program Device (sim)** unchanged | `paint_hw` |

## Before → after

| UX | Before | After |
|----|--------|-------|
| No FTDI/HAD banner | Vague OFL/PATH notes | Explicit **Physical board soft-hold** copy |
| Detect | Full `det.text()` dump | Compact honest **USB / OFL / physical_had** summary |
| Program + usb/native, no probe | Could attempt OFL/native path | **Disabled** with soft-hold message; switch to **sim** |
| Board DONE | never | still **never** |

## Gold

```
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

Verified this turn: `report_timing counter WNS_PS=9640 TNS_PS=0 endpoints=4 r2r_ps=360 iob_ps=220`

## Honesty

- Soft-hold means **no USB programmer evidence** on this host / Mac (Board A). Enumeration alone never equals board DONE.
- Sim / mpsse-sim program paths remain available and label sim fabric only.
- Attach FTDI/HAD → Detect → usb/ofl or native when `physical_had=1`; DONE still requires validated OFL/native STAT evidence.

## Remains

1. Plug FTDI/HAD on Board A → live OFL / `--cable native`  
2. **NO MERGE** until captain says otherwise  

## Verdict

**PASS (UX bar moved).** Program rail honest under physical soft-hold; gold **9640**; no board DONE; Sim layouts untouched; **NO MERGE**.
