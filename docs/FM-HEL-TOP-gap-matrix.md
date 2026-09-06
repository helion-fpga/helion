# FM-HEL-TOP — gap matrix (post-#7)

**Date:** 2026-09-06 ~08:25 America/New_York (EDT)  
**Branch:** `fm-hel-top` — PR https://github.com/helion-fpga/helion/pull/8 (**NO MERGE**; grinding continues)  
**Tip:** `fa91b7f31e28299958d118c30baa1fff7ff5927f` (`fa91b7f`)
**Master:** `46bae17` (PR #7 merge; captain leave-vs-revert)  
**Author:** saksham-45  

## PR #7 note

Helion did **not** merge #7. MergeBy on GitHub: `saksham-45` at 2026-09-06 06:54 ET. Post-merge soft-pass tips (`2714c61`/`358e261`/void PASS) were **not** on master → recovered onto `fm-hel-top`.

## PR #8 grind

**Grinding continues** on `fm-hel-top` / PR #8. **NO MERGE.** **NEVER push soft-pass.**

## Mac sync (e6c67522 / sakshams-MacBook-Pro-517.local)

| Item | Status |
|------|--------|
| Shell | connected |
| Sync | **ff-only** `checkout -B fm-hel-top origin/fm-hel-top` → tip `5a8a107` (no `reset --hard`; chrome clean; only untracked corpus/shots) |
| Air idle | **no `request_repaint(` call sites** in tree (policy strings/comments only); no helion/egui/ftdi/openocd procs |
| FTDI | **soft-hold OK** (USB empty / OFL path unchanged) |

## Track status

| Track | Status | SHA / note |
|-------|--------|------------|
| UX2 void | HARD PASS held | `e521e45` / tip lineage; SPLITTER 6px |
| Schematic deepen | **99 PASS** | `2714c61` (+sha256 wrap); skip ibex uncapped |
| 3 SOFTs | **100/0/0** | verified |
| CLI breadcrumb | shipped | `9ef40e9` |
| Board flash UX | soft-hold + status crumb | `fa91b7f` — `board:soft-hold` on Program; no DONE claim |
| ILA | **mark→impl→arm fixed** | `strip_ila` baseline; see FM-HEL-TOP-ila-ip |
| IP/project | `examples/ip/` + read_ip smoke | dir-form package; AXI fence |
| Air perf | idle-clean | `c5f02b6` + tip reconfirm; `IDLE_PAINT_POLICY=reactive-no-request_repaint`; no request_repaint calls |
| Implement QoR | verified; AIG STOP | gold 9640; reduced cells=2137 imux_skip=0 wall=0.40s — FM-HEL-TOP-impl-quality |
| Live FTDI | soft-hold | Mac USB empty / OFL -3 |

## Gold

```
cargo run -q -p helion-cli -- report_timing examples/counter.sv --sdc examples/counter.sdc
→ WNS_PS=9640  TNS_PS=0  imux_skip=0
```

Verified this turn (box): `report_timing counter WNS_PS=9640 TNS_PS=0 endpoints=4 r2r_ps=360 iob_ps=220`

## Merge

**NO MERGE** until Firstmate/captain.

## Remains

- Live FTDI / board DONE — soft-hold (Mac USB empty); status crumb `board:soft-hold` visible on Program.
- AIG/flowmap wall — STOP (no thrash).
- Captain leave-vs-revert on merged PR #7.
