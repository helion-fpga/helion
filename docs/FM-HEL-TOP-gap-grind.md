# FM-HEL-TOP — gap grind (resume)

**Date:** 2026-09-06 ~08:03 America/New_York (EDT)  
**Branch:** `fm-hel-corpus-soft-pass` (PR #7, **NO MERGE**)  
**Author:** saksham-45 `<72103486+saksham-45@users.noreply.github.com>`  
**CLI breadcrumb SHA:** `9ef40e9d21aabe2d5f0597c45b132e3617b560d8` (`9ef40e9`)  
**Tip HEAD:** `826186afb1f45b4e0746db87179390c48e2b8b86` (`826186a`)  

## Resume status

| Track | Status |
|-------|--------|
| **FM-HEL-UX2** | **HARD PASS** at tip `a5a3b4a` (prior); void/Sim absolute split / `SPLITTER_GRAB_PX=6` / void-class layouts **untouched** this turn |
| **Board A** | **soft-hold** — Mac USB empty / no FTDI (`0x0403`); OFL **-3**; reconfirmed 2026-09-06 ~08:03 EDT (Shell connected; `system_profiler` / `/dev/tty.usb*` / `ioreg` show no Lattice/FTDI) |
| **Letter-led rail** | **already shipped** (48px rail + short labels); chrome assert aligned `side_chrome_width()==268` (48+220) |
| **CLI breadcrumb** | **shipped** this turn — status bar shows `Activity › Canvas` monospace before `part · WNS · LUTFF · run` |
| **Gold** | **WNS_PS=9640** held |
| **Merge** | **NO MERGE** |

## Gold

```
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

Verified this turn: `report_timing counter WNS_PS=9640 TNS_PS=0 endpoints=4 r2r_ps=360 iob_ps=220`

## What shipped (this turn)

| Change | Where |
|--------|--------|
| Status-bar CLI breadcrumb `Files › Editor` (etc.) via `Activity::label` + `Canvas::label` / workspace `canvas_label` | `crates/helion-gui/src/bin/helion-ide.rs` `paint_status_bar` |
| Chrome side-width assert 288→268 (letter-rail math) so floorplan ≥80% fill checks run | `crates/helion-gui/src/chrome.rs` |

## Tests

- `cargo test -p helion-gui --lib -- chrome::` → **3 passed** (overflow+floorplan, letter rail, idle)
- Floorplan fill/gap asserts in chrome overflow test: ≥80% / ≤80px at 800×500 and Mac-like 1152×628
- Do **not** touch Sim absolute split / `SPLITTER_GRAB_PX=6` / void-class layouts

## Breadcrumb (UI)

Status line (monospace, `·`-separated):

```
{Activity} › {Canvas} · {part} · WNS {wns} · LUTFF {lutff} · {run}
```

Example at Files/Editor: `Files › Editor · … · WNS … · LUTFF … · idle`  
Simulate/Wave uses workspace canvas label: `Simulate › Waveform · …`

## Board A (Mac)

- Machine: `sakshams-MacBook-Pro-517.local` connected  
- USB: **empty** for FTDI/Lattice — no `/dev/tty.usb*`, no `0x0403` in tree  
- Live OFL / `--cable native` / STAT TDO: **blocked** (soft-hold, no fake DONE)

## Remains

1. Board A: plug FTDI/HAD → OFL/`--cable native` live path  
2. Optional Mac Device reshot only if captain asks (UX2 already HARD PASS)  
3. **NO MERGE** until captain says otherwise  

## Verdict

**PASS (bar moved).** Breadcrumb tip **`9ef40e9`** (AUTHOR LOCK OK); gold **9640**; UX2 PASS held; board A soft-hold (OFL -3 / no 0x0403); letter rail already done; **NO MERGE**.
