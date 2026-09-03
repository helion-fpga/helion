# Governance

Helion is an **open-source CAD + original FPGA family**. It is not a vendor clone.

## Owner

| Role | Person | GitHub |
|---|---|---|
| Owner | Saksham | [@saksham-45](https://github.com/saksham-45) |

The owner has last say on merges, trademarks, HAD parts, and the legal fence. The GitHub organization is **[helion-fpga](https://github.com/helion-fpga)**; this repository lives at [helion-fpga/helion](https://github.com/helion-fpga/helion). Saksham (@saksham-45) is org owner.

## Maintainers

None yet. Maintainers are named here when they have a track record of reviews and engine-backed patches. See `MAINTAINERS.md`.

## How decisions are made

1. **Code** lands by pull request against `master`. CI + the named test in the PR template.
2. **Gold** for empty-XDC `examples/counter.sv` is `WNS_PS=9640` unless the same commit updates the QoR table **with a reason**.
3. **Legal fence** is not optional: no UNISIM, no vendor Tcl, no Project X-Ray, no 7-series/UltraScale/Lattice/Intel bitstream backends, no AXI interconnect as Helion IP (use Helion-MM/ST).
4. **Issues** are the public backlog. “Good first issue” is for small, engine-backed tasks.

## Sponsors

GitHub Sponsors: see `.github/FUNDING.yml`. Funds go to hosting, domain, and the owner’s time on Helion until a treasurer is named.
