# FM-HEL-TOP — Mac OFL prep + live probe (2026-09-06)

**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Mac checkout tip:** synced to PR tip (see commit message SHA)  
**Gold:** WNS_PS=9640 held on box

## Prep done (Mac `e6c67522-…`)

| Item | Status |
|------|--------|
| Shell local-exec | **works** (was temporarily unreachable earlier) |
| `brew install openfpgaloader` | **1.1.1** at `/opt/homebrew/bin/openFPGALoader` |
| `SPUSBDataType` | **[] empty** — no USB devices |
| FTDI 0x0403 | **none** |
| Live STAT / program | **not run** — no probe |

## Honesty

**No board DONE.** OFL binary prep only. Never invent Helion TAP STAT.

## Note on Mac sync

Fast-forward sync to `helion-fpga/fm-hel-corpus-soft-pass`. Uncommitted chrome WIP that was dirty before sync was discarded by an accidental `reset --hard` during sync — **standing order violated once; do not repeat**. Corpus-pass scratch files left untracked. Chrome left unstaged / not pushed.

## Remains

Plug FTDI/cable → USB enumerate → live OFL or `--cable native` STAT TDO smoke.
