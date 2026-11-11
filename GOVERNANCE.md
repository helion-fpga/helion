# Governance

Helion is an open-source FPGA CAD suite and an original FPGA architecture
(HL10). It is not a clone or a wrapper of a vendor tool. This document says
who makes decisions and how.

## Roles

| Role | Who | Responsibilities |
|---|---|---|
| Owner | Saksham Srivastava ([@saksham-45](https://github.com/saksham-45)) | Final say on merges, releases, the roadmap, the trademark, HAD parts and the legal fence; owner of the [helion-fpga](https://github.com/helion-fpga) organization |
| Maintainers | None yet; see [MAINTAINERS.md](MAINTAINERS.md) | Review and merge pull requests in their areas, triage issues, help cut releases |
| Contributors | Anyone who opens an issue, discussion or pull request | Follow [CONTRIBUTING.md](CONTRIBUTING.md) and the [Code of Conduct](CODE_OF_CONDUCT.md) |

Maintainers are named when they have a track record of reviews and
engine-backed patches. Anyone can ask in
[Discussions](https://github.com/helion-fpga/helion/discussions).

## How decisions are made

1. **Code** lands by pull request against `master`, with green CI and the
   test named in the pull request template. `CODEOWNERS` requests review
   from the owner.
2. **Gold.** The empty-XDC `examples/counter.sv` reports `WNS_PS=9640`. A
   commit that changes a QoR number updates the QoR tests and says why.
3. **Legal fence.** No proprietary vendor IP, libraries or databases;
   vendor-family backends only on open bitstream documentation; no vendor
   trademarks as product names; no AXI interconnect as Helion IP (use
   Helion-MM and Helion-ST). See [ROADMAP.md](ROADMAP.md#legal-fence-every-release).
4. **Larger changes** (new crates, new device families, file-format changes,
   anything that moves the roadmap) start as an issue or Discussion so the
   approach is agreed before the code is written.
5. **Issues** are the public backlog. `good first issue` marks small,
   engine-backed tasks.
6. **Releases** follow [RELEASING.md](RELEASING.md).

## Conduct

Everyone in project spaces follows the [Code of Conduct](CODE_OF_CONDUCT.md).
The owner enforces it until more maintainers are named.

## Sponsors

GitHub Sponsors: see [`.github/FUNDING.yml`](.github/FUNDING.yml). Funds go
to hosting, the domain, and the owner's time on Helion until a treasurer is
named.

## Changes to this document

Changes to governance are made by pull request and approved by the owner.
