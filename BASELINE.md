# FM-HEL-14-P0 BASELINE — Worker 0 inventory

**Date:** 2026-09-14  
**Mac tip (start):** `9997c18` — release: 1.3.0 — pin-wrap, ILA, CDC, Helion catalog  
**Branch (exit):** `wip/1.4-baseline`  
**Isolation:** Mac `~/helion` · saksham-45 only  
**Locks:** empty-XDC `examples/counter.sv` → WNS_PS=9640, LUTFF=4; SOFT ≠ PASS; no UNISIM/X-Ray/vendor Tcl/AXI-as-product; no pkill helion-ide

## Bucket table

| Bucket | Meaning | Disposition |
|--------|---------|-------------|
| **A** | indexes / elab stitch (ir, pack, sta, sv, sim, proj, cli) | **LAND** on `wip/1.4-baseline` first |
| **B** | IDE hang (gui ide.rs / helion-ide.rs / chrome.rs) | **PARK** — do not mix with elab |
| **C** | VHDL (helion-vhdl, rng.vhd, adder4.vhd) | **PARK** — own track |
| **D** | product 1.3 leftovers (bits/debug/device/fabric, HL10M/S, boards) | **PARK** — own track |
| **E** | noise (shots, ibex-cap, corpus-pass, brand media) | **DO NOT COMMIT** |

## Dirty files by bucket

### A — land
- `crates/helion-ir/src/lib.rs` (+219/−…)
- `crates/helion-pack/src/lib.rs`
- `crates/helion-sta/src/lib.rs`
- `crates/helion-sv/src/lib.rs`
- `crates/helion-sim/src/lib.rs`
- `crates/helion-proj/src/lib.rs`
- `crates/helion-cli/src/main.rs`
- `crates/helion-cli/tests/gold_lab.rs` (untracked; A gold harness)

### B — park
- `crates/helion-gui/src/ide.rs` (large +601)
- `crates/helion-gui/src/bin/helion-ide.rs`
- `crates/helion-gui/src/chrome.rs`

### C — park
- `crates/helion-vhdl/src/lib.rs`
- `examples/rng.vhd`
- `examples/adder4.vhd`

### D — park
- `crates/helion-bits/src/lib.rs`
- `crates/helion-debug/src/lib.rs`
- `crates/helion-device/src/lib.rs`
- `crates/helion-fabric/src/lib.rs`
- `devices/helion/parts/HL10M-C128-1.toml`
- `devices/helion/parts/HL10S-C64-1.toml`
- `devices/helion/boards/`

### E — do not commit
- `fm-hel-ux2-shots/`
- `suite-shots/`
- `ibex-cap-full.txt`
- `corpus-pass/`
- `docs/brand/die-orbit*`

## rg hits (shared lock keywords)

See Phase 0 shell log: `WNS_PS=9640`, `no_body`, `push_cell`, `collapse_hierarchy`, `full_adder` under `crates/`.

## Risks
- Mixing B IDE hang with A elab → hang regressions; keep separate.
- Committing E noise bloated history — excluded.
- D HAD parts may be needed later for 1.4 device work — parked, not lost.

## Exit criteria
- [x] BASELINE.md written
- [x] `wip/1.4-baseline` branch
- [x] A-bucket commit `585659e` (saksham-45)
- [x] gold×2: `helion-ide --headless examples/counter.sv` → WNS_PS=9640 twice
- [x] soft-hold / no merge to master

## Gold×2 proof

```
# run1
report_timing counter WNS_PS=9640 ...
# run2
report_timing counter WNS_PS=9640 ...
cargo test -p helion-cli --test gold_lab → 4 passed
```

**Tip after land:** `585659e` on `wip/1.4-baseline` (parent `9997c18`).
