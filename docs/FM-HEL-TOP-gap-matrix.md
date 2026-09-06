# FM-HEL-TOP — gap matrix (post-#7)

**Date:** 2026-09-06 ~08:12 America/New_York (EDT)  
**Branch:** `fm-hel-top` — PR https://github.com/helion-fpga/helion/pull/8 (**NO MERGE**)  
**Tip:** `5cb85d46927149eb5d1fd55e7e437441a9b3d67e` (`5cb85d4`)  
**Master:** `46bae17` (PR #7 merge; captain leave-vs-revert)  
**Author:** saksham-45  

## PR #7 note

Helion did **not** merge #7. MergeBy on GitHub: `saksham-45` at 2026-09-06 06:54 ET. Post-merge soft-pass tips (`2714c61`/`358e261`/void PASS) were **not** on master → recovered onto `fm-hel-top`.

## Track status

| Track | Status | SHA / note |
|-------|--------|------------|
| UX2 void | HARD PASS held | `e521e45` / tip lineage; SPLITTER 6px |
| Schematic deepen | **99 PASS** | `2714c61` (+sha256 wrap); skip ibex uncapped |
| 3 SOFTs | **100/0/0** | verified |
| CLI breadcrumb | shipped | `9ef40e9` |
| Board flash UX | soft-hold physical | this branch `c769aeb` — no DONE claim |
| ILA | crumb + prior `d6f8280` | status crumb on this branch |
| IP/project | `.helion` | `a3da831` (on master via #7) |
| Air perf | idle-clean | `c5f02b6`; no request_repaint |
| Implement QoR | bars moved; AIG STOP | no thrash |
| Live FTDI | soft-hold | Mac USB empty / OFL -3 |

## Gold

```
→ WNS_PS=9640
```

## Merge

**NO MERGE** until Firstmate/captain.
