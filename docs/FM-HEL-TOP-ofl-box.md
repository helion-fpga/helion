# FM-HEL-TOP — OFL dry-run / box install + hw honesty tests (#2)

**Date:** 2026-09-06 ~02:45 America/New_York (UTC-4)  
**Branch:** `fm-hel-corpus-soft-pass` (NO MERGE)  
**Prior:** SHA `ac12bdd` (+ stamp `4603ac1`) — persistent FTDI session + OFL fixture parser  
**Shipped:** SHA `c5f02b6` (`c5f02b6fda7d4b02f193de8ec483d2fdebce4592`)  
**Gold:** `WNS_PS=9640` **held** (`helion report_timing examples/counter.sv --sdc examples/counter.sdc`)

## Goal

Install legal `openFPGALoader` on the Linux box if feasible; dry-run / detect honesty; more `helion-hw` OFL/native tests. Never invent Helion TAP STAT; no fake board DONE.

## OFL install status — **SUCCESS**

| Item | Result |
|------|--------|
| Source | Debian `trixie` apt `openfpgaloader` **0.13.1-1** (official OSS; Homepage github.com/trabucayre/openFPGALoader) |
| Path | `/usr/bin/openFPGALoader` (`0.13.1-1`) |
| Vivado / pirated | **not used** |
| `--scan-usb` | empty / "No USB devices found" + column header (no FTDI on box) |
| `helion-prog --detect` | `ofl path /usr/bin/openFPGALoader`; **`ofl probes 0`**; **`physical_had=0`** |

## Honesty bug fixed (bar move)

OFL 0.13.x prints a **column header** even with zero devices. Prior `parse_scan_usb_output` treated `Bus device vid:pid … probe type …` as a probe → false `physical_had=1` / `ofl probes 1`.

**Fix:** skip `empty` / `No USB…` / `Bus device` header / `vid:pid` without `0x`; require hex VID/PID for a real probe.

| Claim | Before | After (`c5f02b6`) |
|-------|--------|---------------------|
| Box OFL on PATH | no | **yes** (apt 0.13.1) |
| Empty `--scan-usb` | header counted as probe | **0 probes**; physical_had=0 |
| Program w/ 0 probes | (false probe path) | honest **no USB programmer** refuse |
| Helion TAP STAT via OFL | TAP_readback=none | unchanged |
| Board DONE | no | **still no** |

## Tests / fixtures shipped

- Fixtures: `scan_usb_empty.txt` (real OFL empty), `scan_usb_one_ftdi.txt`, `box_ofl_installed_no_probe.txt`
- Tests: `ofl_scan_usb_empty_header_is_zero_probes`, `ofl_real_binary_on_path_detect_zero_probes_no_done`, `ofl_fixture_scan_and_verify_never_invent_helion_stat`, `native_mpsse_and_ofl_honesty_coexist_on_box`
- `cargo test -p helion-hw --lib --features usb-native` → **41 passed**

## Live probe

| Check | Result |
|-------|--------|
| Box FTDI | **0** |
| Mac `e6c67522-…` | listed connected; Shell **temporarily unreachable** (one attempt) |
| Board DONE | **no** |

## Evidence

```
sudo apt-get install -y openfpgaloader   # 0.13.1-1
openFPGALoader --scan-usb                # empty + header only
helion-prog --detect --features usb-native
# → physical_had=0; ofl probes 0; ofl path /usr/bin/openFPGALoader
cargo test -p helion-hw --lib --features usb-native   # 41 passed
helion report_timing examples/counter.sv --sdc examples/counter.sdc
# → WNS_PS=9640
```

## What remains

1. Mac local-exec + real FTDI/HAD: live OFL program / native MPSSE session.
2. Upstream OFL `helion_hl10t` board alias (still Helion-local `HELION_OFL_BOARD`).
3. Do **not** claim board DONE without USB/OFL hardware evidence.

## Verdict

**PASS (bar moved; hardware still blocked)** — OFL installed on box; scan-usb header honesty fixed; dry-run/detect/fixture tests expanded; gold 9640; no merge.

## macOS (Homebrew) — OFL prep

Legal OSS only (`openfpgaloader` Apache-2.0 on homebrew-core). Do **not** claim board DONE from install alone.

```bash
brew install openfpgaloader   # bottle ~1.1.x + libftdi/libusb
which openFPGALoader          # expect /opt/homebrew/bin/openFPGALoader
openFPGALoader --scan-usb     # empty / No USB → 0 probes (honest)
```

Captain Mac (2026-09-06): brew install succeeded (`openfpgaloader` 1.1.1); `SPUSBDataType` empty; `--scan-usb` / `--detect` → no FTDI (`unable to open ftdi device: -3`). **No board DONE.**

