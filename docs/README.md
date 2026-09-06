# Helion site

Static pages for GitHub Pages (`docs/` → https://helion-fpga.github.io/helion/).

| Page | Job |
|---|---|
| `index.html` | Landing: Helion mark, gold WNS, call for help |
| `start.html` | Build / gold / macOS app |
| `use.html` | User guide (IDE, CLI, Device die, `.helion` IP, program honesty) |
| `wiki.html` | Wiki index |
| `architecture.html` | Crate map and labeled HAD |
| `get-involved.html` | LMMS-style on-ramp |
| `contribute.html` | Coding path |
| `legal.html` | Fence |

Deploy: `.github/workflows/pages.yml` on push to `master` when `docs/` changes.

## Developer crumbs (FM-HEL-TOP notes)

Branch work notes (not the public on-ramp). Gold hold: **WNS_PS=9640**. Cap holds: **imux_skip=0**, **IOB=1**.

| Note | What |
|---|---|
| [`FM-HEL-TOP-helion-ip-package.md`](FM-HEL-TOP-helion-ip-package.md) | Format-1 `.helion` + `read_ip` |
| [`FM-HEL-TOP-ofl-box.md`](FM-HEL-TOP-ofl-box.md) | Box OFL install + scan-usb header honesty |
| [`FM-HEL-TOP-tap-ibex-smoke.md`](FM-HEL-TOP-tap-ibex-smoke.md) | Ibex mpsse-sim STAT smoke |
| [`FM-HEL-TOP-place-legalize-speed.md`](FM-HEL-TOP-place-legalize-speed.md) | Affinity place ~3s→~0.23s |
| [`FM-HEL-TOP-synth-wall.md`](FM-HEL-TOP-synth-wall.md) | keep/md + STA PinIndex → ~1.40s pin-wrap |
| [`FM-HEL-TOP-aig-flowmap-residual.md`](FM-HEL-TOP-aig-flowmap-residual.md) | AIG/flowmap residual **STOP** (no cheap wall cut) |
| [`FM-HEL-TOP-air-budgets.md`](FM-HEL-TOP-air-budgets.md) | Air idle budgets |
| [`ip/README.md`](../ip/README.md) | `.helion` package how-to |

Also: native/live FTDI/OFL, IMUX reach, soft Helion-ahead — same `FM-HEL-TOP-*.md` prefix.
